use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::manifest::Output;

pub trait FrameSink: Send {
    fn write_frame(&mut self, yuyv: &[u8]) -> Result<()>;
}

const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_FIELD_NONE: u32 = 1;
const V4L2_COLORSPACE_SRGB: u32 = 8;
const V4L2_PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");

#[repr(C)]
#[derive(Default)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

/// `struct v4l2_format`: a 4-byte type, then a 200-byte union aligned to 8 on 64-bit.
#[repr(C)]
struct V4l2Format {
    type_: u32,
    _pad: u32,
    pix: V4l2PixFormat,
    _rest: [u8; 200 - std::mem::size_of::<V4l2PixFormat>()],
}

const _: () = assert!(std::mem::size_of::<V4l2Format>() == 208);

nix::ioctl_readwrite!(vidioc_s_fmt, b'V', 5, V4l2Format);

/// Writes YUYV frames to a v4l2loopback output device.
pub struct V4l2Sink {
    file: File,
}

impl V4l2Sink {
    pub fn open(path: &Path, o: Output) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .with_context(|| format!("opening {} (run scripts/setup-vcam.sh)", path.display()))?;
        let mut fmt = V4l2Format {
            type_: V4L2_BUF_TYPE_VIDEO_OUTPUT,
            _pad: 0,
            pix: V4l2PixFormat {
                width: o.width,
                height: o.height,
                pixelformat: V4L2_PIX_FMT_YUYV,
                field: V4L2_FIELD_NONE,
                bytesperline: o.width * 2,
                sizeimage: o.width * o.height * 2,
                colorspace: V4L2_COLORSPACE_SRGB,
                ..Default::default()
            },
            _rest: [0; 200 - std::mem::size_of::<V4l2PixFormat>()],
        };
        // SAFETY: fmt is a correctly sized v4l2_format that outlives the call.
        unsafe { vidioc_s_fmt(file.as_raw_fd(), &mut fmt) }.context("VIDIOC_S_FMT on the loopback device")?;
        if fmt.pix.width != o.width || fmt.pix.height != o.height || fmt.pix.pixelformat != V4L2_PIX_FMT_YUYV {
            bail!(
                "device negotiated {}x{} fourcc {:#x}; another writer may hold a different format",
                fmt.pix.width,
                fmt.pix.height,
                fmt.pix.pixelformat
            );
        }
        Ok(Self { file })
    }
}

impl FrameSink for V4l2Sink {
    fn write_frame(&mut self, yuyv: &[u8]) -> Result<()> {
        self.file.write_all(yuyv)?;
        Ok(())
    }
}

/// Discards frames, for running without a camera device.
pub struct NullSink;

impl FrameSink for NullSink {
    fn write_frame(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}

/// Encodes frames to a video file through ffmpeg, optionally muxing an audio track.
pub struct FfmpegSink {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl FfmpegSink {
    pub fn create(path: &Path, o: Output, audio: Option<&Path>) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "yuyv422"])
            .args(["-s", &format!("{}x{}", o.width, o.height), "-r", &o.fps.to_string(), "-i", "-"]);
        if let Some(a) = audio {
            cmd.arg("-i").arg(a).args(["-c:a", "aac", "-shortest"]);
        }
        cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"]).arg(path);
        let mut child = cmd.stdin(Stdio::piped()).spawn().context("spawning ffmpeg encoder")?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        if !status.success() {
            bail!("ffmpeg encoder exited with {status}");
        }
        Ok(())
    }
}

impl FrameSink for FfmpegSink {
    fn write_frame(&mut self, yuyv: &[u8]) -> Result<()> {
        self.stdin.as_mut().expect("open until finish").write_all(yuyv)?;
        Ok(())
    }
}
