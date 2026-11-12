//! Microphone noise suppression: RNNoise (the `nnnoiseless` port, model built
//! in). It runs when a clip is saved, on that clip's microphone audio only, so
//! it costs nothing while Glint is just buffering.
use crate::audio::Pcm;
use nnnoiseless::DenoiseState;
use std::sync::Mutex;

/// RNNoise works on 10 ms frames of 48 kHz mono.
const FRAME: usize = DenoiseState::FRAME_SIZE;
const RATE: u32 = 48_000;
/// Long clips are cut into pieces that are cleaned on separate threads. Each
/// piece first runs over the second before it so the network has settled.
const CHUNK: usize = FRAME * 1500; // 15 s
const PREROLL: usize = FRAME * 100; // 1 s

/// Clean a stereo s16 buffer in place. The result is mono on both channels: a
/// voice microphone is one source, and RNNoise is a single-channel model.
pub fn process(pcm: &mut Pcm) {
    let frames = pcm.data.len() / 2;
    if frames < FRAME || pcm.rate == 0 {
        return;
    }
    let mono: Vec<f32> = pcm.data.chunks_exact(2).map(|s| (s[0] as f32 + s[1] as f32) * 0.5).collect();
    let input = if pcm.rate == RATE { mono } else { resample(&mono, pcm.rate, RATE) };
    let mut out = vec![0f32; input.len()];
    {
        let jobs: Mutex<Vec<(usize, &mut [f32])>> = Mutex::new(out.chunks_mut(CHUNK).enumerate().map(|(i, c)| (i * CHUNK, c)).collect());
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 8);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let job = jobs.lock().unwrap().pop();
                    let Some((start, dst)) = job else { break };
                    clean(&input, start, dst);
                });
            }
        });
    }
    let out = if pcm.rate == RATE { out } else { resample(&out, RATE, pcm.rate) };
    for (i, s) in pcm.data.chunks_exact_mut(2).enumerate() {
        let v = out.get(i).copied().unwrap_or(0.0).clamp(-32768.0, 32767.0) as i16;
        s[0] = v;
        s[1] = v;
    }
}

/// Fill `dst` with the cleaned version of `input[start .. start + dst.len()]`.
fn clean(input: &[f32], start: usize, dst: &mut [f32]) {
    let mut state = DenoiseState::new();
    let mut frame = [0f32; FRAME];
    let mut cleaned = [0f32; FRAME];
    let end = start + dst.len();
    let mut pos = start.saturating_sub(PREROLL);
    // RNNoise answers one frame late: the output of a call is the cleaned
    // version of the frame given in the call before. Each output is therefore
    // written one frame back, and one extra frame is pushed through at the end.
    while pos < end + FRAME {
        frame.fill(0.0);
        if pos < input.len() {
            let n = FRAME.min(input.len() - pos);
            frame[..n].copy_from_slice(&input[pos..pos + n]);
        }
        state.process_frame(&mut cleaned, &frame);
        if pos >= start + FRAME {
            let at = pos - FRAME - start;
            if at < dst.len() {
                let n = FRAME.min(dst.len() - at);
                dst[at..at + n].copy_from_slice(&cleaned[..n]);
            }
        }
        pos += FRAME;
    }
}

/// Linear resampling. Enough for a voice track on its way through a speech model.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if x.is_empty() || from == to {
        return x.to_vec();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let a = p.floor() as usize;
            let frac = (p - a as f64) as f32;
            let s0 = x[a.min(x.len() - 1)];
            let s1 = x[(a + 1).min(x.len() - 1)];
            s0 + (s1 - s0) * frac
        })
        .collect()
}
