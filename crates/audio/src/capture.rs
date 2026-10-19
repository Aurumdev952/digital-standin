use std::time::Duration;

use anyhow::{Result, anyhow};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::pod::Pod;

use crate::{Fanout, SAMPLE_RATE};

/// Captures the monitor of whatever the default output sink is, following it when it changes.
///
/// A PipeWire sink monitor carries the signal before the sink's volume and mute are applied, so
/// turning the speakers down does not silence the capture. PipeWire's adapter resamples and
/// downmixes to 16 kHz mono for us.
pub fn capture_default_sink(mut fanout: Fanout) -> Result<()> {
    let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(SAMPLE_RATE as usize * 4);
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);

    std::thread::Builder::new().name("pipewire".into()).spawn(move || {
        let run = || -> Result<(), pw::Error> {
            pw::init();
            let mainloop = pw::main_loop::MainLoopRc::new(None)?;
            let context = pw::context::ContextRc::new(&mainloop, None)?;
            let core = context.connect_rc(None)?;
            let props = properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::STREAM_CAPTURE_SINK => "true",
                *pw::keys::NODE_NAME => "digital-standin-listener",
                *pw::keys::NODE_LATENCY => "320/16000",
            };
            let stream = pw::stream::StreamBox::new(&core, "digital-standin-listener", props)?;
            let _listener = stream
                .add_local_listener::<()>()
                .process(move |stream, _| {
                    let Some(mut buffer) = stream.dequeue_buffer() else { return };
                    let Some(data) = buffer.datas_mut().first_mut() else { return };
                    let n = data.chunk().size() as usize / 4;
                    let Some(bytes) = data.data() else { return };
                    for b in bytes[..n * 4].chunks_exact(4) {
                        let _ = producer.push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                    }
                })
                .register()?;

            let mut info = spa::param::audio::AudioInfoRaw::new();
            info.set_format(spa::param::audio::AudioFormat::F32LE);
            info.set_rate(SAMPLE_RATE);
            info.set_channels(1);
            let obj = spa::pod::Object {
                type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                id: spa::param::ParamType::EnumFormat.as_raw(),
                properties: info.into(),
            };
            let bytes = spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &spa::pod::Value::Object(obj),
            )
            .expect("serialize format pod")
            .0
            .into_inner();
            let mut params = [Pod::from_bytes(&bytes).expect("valid pod")];
            stream.connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )?;
            let _ = ready_tx.send(Ok(()));
            mainloop.run();
            Ok(())
        };
        if let Err(e) = run() {
            let _ = ready_tx.send(Err(e.to_string()));
        }
    })?;

    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow!("PipeWire did not respond within 5 s"))?
        .map_err(|e| anyhow!("PipeWire capture failed: {e}"))?;

    std::thread::Builder::new().name("audio-fanout".into()).spawn(move || {
        let mut buf = Vec::with_capacity(SAMPLE_RATE as usize);
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let n = consumer.slots();
            if n == 0 {
                continue;
            }
            let chunk = consumer.read_chunk(n).expect("n readable slots");
            let (a, b) = chunk.as_slices();
            buf.clear();
            buf.extend_from_slice(a);
            buf.extend_from_slice(b);
            chunk.commit_all();
            fanout.push(&buf);
        }
    })?;
    Ok(())
}
