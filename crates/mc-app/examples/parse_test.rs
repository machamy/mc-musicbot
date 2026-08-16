//! 재생 파이프라인 재현 테스트 (디스코드 연결 없이).
//! ffmpeg → ChildContainer → RawAdapter → symphonia promote + 디코드까지.
//! 사용: cargo run --release --example parse_test -- <audio-file> [s16le|f32le]

use songbird::input::core::io::ReadOnlySource;
use songbird::input::{AudioStream, LiveInput, RawAdapter, codecs};
use std::process::Stdio;

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("usage: parse_test <file> [s16le|f32le]");
    let fmt = args.next().unwrap_or_else(|| "s16le".into());

    let child = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &file,
            "-af",
            "dynaudnorm=f=200:g=15",
            "-ac",
            "2",
            "-f",
            &fmt,
            "-ar",
            "48000",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("ffmpeg spawn");

    let container = songbird::input::ChildContainer::new(vec![child]);
    let adapter = RawAdapter::new(ReadOnlySource::new(container), 48000, 2);
    let stream = AudioStream {
        input: Box::new(adapter) as Box<dyn songbird::input::core::io::MediaSource>,
    };
    let live = LiveInput::Raw(stream);

    println!("[1] promoting (format={fmt})...");
    let promoted = live.promote(codecs::get_codec_registry(), codecs::get_probe());
    let mut parsed_live = match promoted {
        Ok(p) => {
            println!("[1] promote OK, playable={}", p.is_playable());
            p
        }
        Err(e) => {
            println!("[1] promote FAILED: {e:?}");
            return;
        }
    };

    let parsed = parsed_live.parsed_mut().expect("parsed");
    println!("[2] reading packets + decoding...");
    let mut total_frames: u64 = 0;
    let mut peak: f32 = 0.0;
    for i in 0..200 {
        match parsed.format.next_packet() {
            Ok(pkt) => match parsed.decoder.decode(&pkt) {
                Ok(buf) => {
                    use songbird::input::core::audio::Signal;
                    let spec_frames = match &buf {
                        songbird::input::core::audio::AudioBufferRef::F32(b) => {
                            for ch in 0..b.spec().channels.count() {
                                for s in b.chan(ch) {
                                    let a = s.abs();
                                    if a > peak {
                                        peak = a;
                                    }
                                }
                            }
                            b.frames() as u64
                        }
                        other => {
                            println!(
                                "  packet {i}: non-f32 buffer variant ({} frames?)",
                                other.frames()
                            );
                            other.frames() as u64
                        }
                    };
                    total_frames += spec_frames;
                }
                Err(e) => {
                    println!("[2] DECODE ERROR at packet {i}: {e:?}");
                    return;
                }
            },
            Err(e) => {
                println!(
                    "[2] next_packet end/error at {i}: {e:?} (total_frames={total_frames}, peak={peak})"
                );
                return;
            }
        }
    }
    println!("[2] OK — 200 packets, total_frames={total_frames}, peak_amplitude={peak}");
}
