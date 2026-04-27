use std::{
    cell::RefCell,
    num::NonZeroUsize,
    os::fd::{BorrowedFd, OwnedFd},
    path::PathBuf,
    rc::Rc,
};

use image::{ImageBuffer, Rgba, RgbaImage};
use nix::sys::mman::{MapFlags, MmapAdvise, ProtFlags};
use pipewire as pw;
use pw::{
    main_loop::MainLoopWeak,
    properties::properties,
    spa::{
        buffer::DataType,
        param::video::{VideoFormat, VideoInfoRaw},
    },
    stream::StreamFlags,
};

use crate::domain::{
    config::Config,
    error::{AppError, Result},
};

/// Quit the PipeWire main loop after this many consecutive undecoded frames
/// to prevent an indefinite hang when the stream delivers unusable buffers.
const MAX_FAILED_FRAMES: u32 = 10;

struct UserData {
    format: VideoInfoRaw,
    image: Rc<RefCell<Option<RgbaImage>>>,
    ml_weak: MainLoopWeak,
    error: Rc<RefCell<Option<String>>>,
    failed_frames: u32,
}

pub fn capture(cfg: &Config) -> Result<PathBuf> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;

    let (node_id, fd) = rt.block_on(open_portal())?;

    // Use a true OS thread so PipeWire's main loop is fully isolated from the
    // tokio reactor — they both use signal handlers and thread-local state that
    // must not be shared.
    let image = std::thread::spawn(move || pipewire_capture(node_id, fd))
        .join()
        .map_err(|_| AppError::Other("PipeWire capture thread panicked".into()))??;

    let path = cfg.output_path();
    image.save(&path).map_err(AppError::from)?;
    Ok(path)
}

async fn open_portal() -> Result<(u32, OwnedFd)> {
    use ashpd::desktop::{
        PersistMode,
        screencast::{
            CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
        },
    };

    let proxy: Screencast = Screencast::new()
        .await
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?;

    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?
        .response()
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?;

    let stream = response
        .streams()
        .first()
        .ok_or(AppError::UserCancelled)?
        .clone();

    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .map_err(|e: ashpd::Error| AppError::Other(e.to_string()))?;

    Ok((stream.pipe_wire_node_id(), fd))
}

fn pipewire_capture(node_id: u32, fd: OwnedFd) -> Result<RgbaImage> {
    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| AppError::Other(e.to_string()))?;
    let context =
        pw::context::ContextRc::new(&mainloop, None).map_err(|e| AppError::Other(e.to_string()))?;
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|e| AppError::Other(e.to_string()))?;

    let image_cell: Rc<RefCell<Option<RgbaImage>>> = Rc::new(RefCell::new(None));
    let ml_weak = mainloop.downgrade();

    let error_cell: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let user_data = UserData {
        format: Default::default(),
        image: image_cell.clone(),
        ml_weak,
        error: error_cell.clone(),
        failed_frames: 0,
    };

    let stream = pw::stream::StreamRc::new(
        core,
        "hyprcrop-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| AppError::Other(e.to_string()))?;

    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .param_changed(|_, ud, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, ms)) = pw::spa::param::format_utils::parse_format(param) else {
                return;
            };
            if mt != pw::spa::param::format::MediaType::Video
                || ms != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let _ = ud.format.parse(param);
        })
        .state_changed(|_stream, ud, _old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                *ud.error.borrow_mut() = Some(msg);
                if let Some(ml) = ud.ml_weak.upgrade() {
                    ml.quit();
                }
            }
        })
        .process(|stream, ud| {
            let Some(mut buf) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buf.datas_mut();
            if datas.is_empty() {
                return;
            }

            // Safety: access the raw spa_data to check chunk pointer before
            // dereferencing it. A null chunk causes a panic inside chunk()
            // which, crossing the C FFI boundary, becomes SIGSEGV.
            let raw = datas[0].as_raw();
            if raw.chunk.is_null() {
                return;
            }
            let dt = datas[0].type_();
            let (chunk_offset, chunk_size, stride) = {
                let chunk = datas[0].chunk();
                (
                    chunk.offset() as usize,
                    chunk.size() as usize,
                    chunk.stride(),
                )
            };
            if chunk_size == 0 {
                return;
            }

            let maybe_img = match dt {
                DataType::MemPtr => {
                    // Data pointer already mapped by PipeWire.
                    datas[0].data().and_then(|d: &mut [u8]| {
                        let end = (chunk_offset + chunk_size).min(d.len());
                        let frame = &d[chunk_offset.min(d.len())..end];
                        let w = ud.format.size().width;
                        let h = ud.format.size().height;
                        decode_frame(frame, w, h, stride as u32, ud.format.format())
                    })
                }
                DataType::MemFd => {
                    // Manually mmap the memfd. Must use spa_data.mapoffset
                    // (page-aligned offset into the fd) and spa_data.maxsize
                    // (total map size). Using chunk values here underestimates
                    // the map size and causes SIGSEGV on access.
                    let map_size = raw.maxsize as usize;
                    let map_offset = raw.mapoffset as i64;
                    let raw_fd = raw.fd as i32;

                    // spa_data.fd == -1 means no fd is set; constructing a
                    // BorrowedFd from -1 is immediate UB at the Rust level.
                    // Additionally verify the fd is actually open before
                    // borrowing it, guarding against stale descriptors.
                    if raw_fd < 0 {
                        None
                    } else if nix::fcntl::fcntl(unsafe { BorrowedFd::borrow_raw(raw_fd) }, nix::fcntl::FcntlArg::F_GETFD).is_err() {
                        eprintln!("[hyprcrop] warning: memfd descriptor {raw_fd} is invalid, skipping frame");
                        None
                    } else {
                        NonZeroUsize::new(map_size.max(1)).and_then(|len| {
                            let bfd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
                            let ptr = unsafe {
                                nix::sys::mman::mmap(
                                    None,
                                    len,
                                    ProtFlags::PROT_READ,
                                    MapFlags::MAP_SHARED,
                                    bfd,
                                    map_offset,
                                )
                                .ok()?
                            };
                            let _ = unsafe {
                                nix::sys::mman::madvise(ptr, len.get(), MmapAdvise::MADV_SEQUENTIAL)
                            };
                            let slice = unsafe {
                                std::slice::from_raw_parts(ptr.as_ptr().cast::<u8>(), len.get())
                            };
                            let end = (chunk_offset + chunk_size).min(len.get());
                            let frame = &slice[chunk_offset.min(len.get())..end];
                            let w = ud.format.size().width;
                            let h = ud.format.size().height;
                            let img = decode_frame(frame, w, h, stride as u32, ud.format.format());
                            if let Err(e) = unsafe { nix::sys::mman::munmap(ptr, len.get()) } {
                                eprintln!("[hyprcrop] warning: munmap failed: {e}");
                            }
                            img
                        })
                    }
                }
                _ => None,
            };

            if let Some(img) = maybe_img {
                *ud.image.borrow_mut() = Some(img);
                if let Some(ml) = ud.ml_weak.upgrade() {
                    ml.quit();
                }
            } else {
                // Guard against an indefinite hang when the stream delivers
                // buffers that cannot be decoded (unsupported format, mmap
                // failure, zero stride, etc.).
                ud.failed_frames += 1;
                if ud.failed_frames >= MAX_FAILED_FRAMES {
                    *ud.error.borrow_mut() =
                        Some("stream delivered too many undecodable frames".into());
                    if let Some(ml) = ud.ml_weak.upgrade() {
                        ml.quit();
                    }
                }
            }
        })
        .register()
        .map_err(|e| AppError::Other(e.to_string()))?;

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRA,
            VideoFormat::BGRx,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 7680,
                height: 4320
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 1, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );

    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| AppError::Other(e.to_string()))?
    .0
    .into_inner();

    let mut params = [pw::spa::pod::Pod::from_bytes(&values)
        .ok_or_else(|| AppError::Other("Failed to build PipeWire params pod".to_string()))?];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT,
            &mut params,
        )
        .map_err(|e| AppError::Other(e.to_string()))?;

    mainloop.run();

    drop(_listener);

    if let Some(err) = error_cell.borrow_mut().take() {
        return Err(AppError::Other(format!("PipeWire stream error: {err}")));
    }

    image_cell
        .borrow_mut()
        .take()
        .ok_or_else(|| AppError::Other("Portal capture yielded no frame".to_string()))
}

fn decode_frame(data: &[u8], w: u32, h: u32, stride: u32, fmt: VideoFormat) -> Option<RgbaImage> {
    if w == 0 || h == 0 {
        return None;
    }
    let min_stride = w as usize * 4;
    // stride == 0 means tightly packed; treat as w*4.
    // stride < min_stride means rows overlap — reject to avoid corrupted output.
    let stride = match stride as usize {
        0 => min_stride,
        s if s < min_stride => return None,
        s => s,
    };
    let mut img: RgbaImage = ImageBuffer::new(w, h);
    for row in 0..h as usize {
        let row_start = row * stride;
        let row_end = row_start + (w as usize * 4);
        if row_end > data.len() {
            return None;
        }
        let src = &data[row_start..row_end];
        for col in 0..w as usize {
            let base = col * 4;
            let pixel = match fmt {
                // BGRA → RGBA
                VideoFormat::BGRA => Rgba([src[base + 2], src[base + 1], src[base], src[base + 3]]),
                // BGRx → RGBA (x → 255)
                VideoFormat::BGRx => Rgba([src[base + 2], src[base + 1], src[base], 255]),
                // RGBA → RGBA (pass-through)
                VideoFormat::RGBA => Rgba([src[base], src[base + 1], src[base + 2], src[base + 3]]),
                // RGBx → RGBA
                VideoFormat::RGBx => Rgba([src[base], src[base + 1], src[base + 2], 255]),
                _ => return None,
            };
            img.put_pixel(col as u32, row as u32, pixel);
        }
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::decode_frame;
    use image::Rgba;
    use pipewire::spa::param::video::VideoFormat;

    #[test]
    fn decode_frame_converts_bgra() {
        let data = [10u8, 20, 30, 40];
        let img = decode_frame(&data, 1, 1, 4, VideoFormat::BGRA).expect("BGRA decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([30, 20, 10, 40]));
    }

    #[test]
    fn decode_frame_converts_bgrx() {
        let data = [10u8, 20, 30, 99];
        let img = decode_frame(&data, 1, 1, 4, VideoFormat::BGRx).expect("BGRx decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([30, 20, 10, 255]));
    }

    #[test]
    fn decode_frame_converts_rgba() {
        let data = [1u8, 2, 3, 4];
        let img = decode_frame(&data, 1, 1, 4, VideoFormat::RGBA).expect("RGBA decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([1, 2, 3, 4]));
    }

    #[test]
    fn decode_frame_converts_rgbx() {
        let data = [5u8, 6, 7, 200];
        let img = decode_frame(&data, 1, 1, 4, VideoFormat::RGBx).expect("RGBx decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([5, 6, 7, 255]));
    }

    #[test]
    fn decode_frame_respects_stride_padding() {
        // stride=8 means 4 pixels-per-row + 4 bytes padding; only 1 pixel wide
        let data = [
            1u8, 2, 3, 4, 9, 9, 9, 9, // row 0: pixel + padding
            5, 6, 7, 8, 8, 8, 8, 8, // row 1: pixel + padding
        ];
        let img = decode_frame(&data, 1, 2, 8, VideoFormat::RGBA).expect("stride decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([1, 2, 3, 4]));
        assert_eq!(*img.get_pixel(0, 1), Rgba([5, 6, 7, 8]));
    }

    #[test]
    fn decode_frame_rejects_zero_dimensions() {
        assert!(decode_frame(&[1, 2, 3, 4], 0, 1, 4, VideoFormat::RGBA).is_none());
        assert!(decode_frame(&[1, 2, 3, 4], 1, 0, 4, VideoFormat::RGBA).is_none());
    }

    #[test]
    fn decode_frame_rejects_unsupported_format() {
        assert!(decode_frame(&[1, 2, 3, 4], 1, 1, 4, VideoFormat::Unknown).is_none());
    }

    #[test]
    fn decode_frame_rejects_insufficient_row_data() {
        assert!(decode_frame(&[1, 2, 3], 1, 1, 4, VideoFormat::RGBA).is_none());
    }

    #[test]
    fn decode_frame_rejects_short_second_row() {
        // stride=8, w=1, h=2 → row 1 needs bytes 8..12; 11 bytes is insufficient
        let data = [1u8, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7];
        assert!(decode_frame(&data, 1, 2, 8, VideoFormat::RGBA).is_none());
    }

    #[test]
    fn decode_frame_zero_stride_treated_as_packed() {
        // stride=0 should be treated as w*4 (tightly packed)
        let data = [10u8, 20, 30, 40];
        let img = decode_frame(&data, 1, 1, 0, VideoFormat::RGBA).expect("zero stride decode");
        assert_eq!(*img.get_pixel(0, 0), Rgba([10, 20, 30, 40]));
    }

    #[test]
    fn decode_frame_rejects_stride_smaller_than_row() {
        // stride=3 is smaller than w*4=4 — rows would overlap
        assert!(decode_frame(&[1, 2, 3, 4], 1, 1, 3, VideoFormat::RGBA).is_none());
    }
}
