//! H.264 from RTP (RFC 6184) to JPEG frames.
//!
//! [`Depacketizer`] turns RTP packets into whole access units in Annex B
//! form (start codes before each NAL unit). [`JpegWorker`] decodes them with
//! OpenH264 on its own thread, so decoding never blocks the async runtime,
//! and encodes a JPEG at most once per frame interval.
//!
//! Every access unit is decoded, because each P-frame depends on the frames
//! before it and the X1's keyframe interval is unknown. The costly
//! conversion to RGB and the JPEG encode happen only for frames sent on,
//! which is what keeps the CPU cost down on a Raspberry Pi (ADR-0007).
use std::time::{Duration, Instant};

use bytes::Bytes;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use tokio::sync::mpsc;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
const STAP_A: u8 = 24;
const FU_A: u8 = 28;

/// The longest access unit accepted, so a broken stream can't use up memory.
const MAX_ACCESS_UNIT: usize = 8 << 20;

/// Reassembles access units from RTP packets.
#[derive(Debug, Default)]
pub struct Depacketizer {
    /// SPS and PPS from the SDP, put before each keyframe that lacks them.
    parameter_sets: Vec<Vec<u8>>,
    nals: Vec<Vec<u8>>,
    fragment: Option<Vec<u8>>,
    last_sequence: Option<u16>,
    /// After a lost packet, nothing is passed on until the next keyframe.
    waiting_for_keyframe: bool,
}

impl Depacketizer {
    pub fn new(parameter_sets: Vec<Vec<u8>>) -> Self {
        Self {
            parameter_sets,
            waiting_for_keyframe: true,
            ..Self::default()
        }
    }

    /// Takes one RTP packet. Returns an access unit when this packet ends one
    /// (the RTP marker bit).
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let (payload, marker, sequence) = rtp_payload(packet)?;
        if let Some(last) = self.last_sequence
            && sequence != last.wrapping_add(1)
        {
            // Lost (or reordered) packets: drop the partial unit.
            self.nals.clear();
            self.fragment = None;
            self.waiting_for_keyframe = true;
        }
        self.last_sequence = Some(sequence);
        if payload.is_empty() {
            return None;
        }
        match payload[0] & 0x1F {
            1..=23 => self.nals.push(payload.to_vec()),
            STAP_A => {
                let mut rest = &payload[1..];
                while rest.len() >= 2 {
                    let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
                    let Some(nal) = rest.get(2..2 + len) else {
                        break;
                    };
                    if !nal.is_empty() {
                        self.nals.push(nal.to_vec());
                    }
                    rest = &rest[2 + len..];
                }
            }
            FU_A if payload.len() >= 2 => {
                let header = payload[1];
                let (start, end) = (header & 0x80 != 0, header & 0x40 != 0);
                if start {
                    let mut nal = vec![(payload[0] & 0xE0) | (header & 0x1F)];
                    nal.extend_from_slice(&payload[2..]);
                    self.fragment = Some(nal);
                } else if let Some(nal) = self.fragment.as_mut() {
                    nal.extend_from_slice(&payload[2..]);
                    if nal.len() > MAX_ACCESS_UNIT {
                        // Garbled: never let one fragment grow without bound.
                        self.fragment = None;
                    }
                }
                if end && let Some(nal) = self.fragment.take() {
                    self.nals.push(nal);
                }
            }
            // STAP-B, MTAP and FU-B are not used in packetization mode 1.
            _ => {}
        }
        if self.nals.iter().map(Vec::len).sum::<usize>() > MAX_ACCESS_UNIT {
            self.nals.clear();
            self.waiting_for_keyframe = true;
        }
        if !marker || self.nals.is_empty() {
            return None;
        }
        let nals = std::mem::take(&mut self.nals);
        let keyframe = nals.iter().any(|n| n[0] & 0x1F == NAL_IDR);
        if self.waiting_for_keyframe && !keyframe {
            return None;
        }
        self.waiting_for_keyframe = false;
        let has_sps = nals.iter().any(|n| n[0] & 0x1F == NAL_SPS);
        let has_pps = nals.iter().any(|n| n[0] & 0x1F == NAL_PPS);
        // Keep the newest in-band parameter sets for later keyframes.
        if has_sps && has_pps {
            self.parameter_sets = nals
                .iter()
                .filter(|n| matches!(n[0] & 0x1F, NAL_SPS | NAL_PPS))
                .cloned()
                .collect();
        }
        let mut unit = Vec::new();
        if keyframe && !(has_sps && has_pps) {
            for set in &self.parameter_sets {
                unit.extend_from_slice(&START_CODE);
                unit.extend_from_slice(set);
            }
        }
        for nal in &nals {
            unit.extend_from_slice(&START_CODE);
            unit.extend_from_slice(nal);
        }
        Some(unit)
    }
}

/// An RTP packet's payload, marker bit and sequence number.
fn rtp_payload(packet: &[u8]) -> Option<(&[u8], bool, u16)> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let padding = packet[0] & 0x20 != 0;
    let extension = packet[0] & 0x10 != 0;
    let csrcs = usize::from(packet[0] & 0x0F);
    let marker = packet[1] & 0x80 != 0;
    let sequence = u16::from_be_bytes([packet[2], packet[3]]);
    let mut start = 12 + csrcs * 4;
    if extension {
        let words = usize::from(u16::from_be_bytes([
            *packet.get(start + 2)?,
            *packet.get(start + 3)?,
        ]));
        start += 4 + words * 4;
    }
    let mut end = packet.len();
    if padding {
        end = end.checked_sub(usize::from(*packet.last()?))?;
    }
    (start <= end).then(|| (&packet[start..end], marker, sequence))
}

/// Decodes access units on its own thread and sends JPEGs on, at most one
/// per `interval`. The thread ends when the worker is dropped.
pub struct JpegWorker {
    units: mpsc::Sender<Vec<u8>>,
}

impl JpegWorker {
    pub fn spawn(
        interval: Duration,
        quality: u8,
        jpegs: mpsc::Sender<Bytes>,
    ) -> std::io::Result<Self> {
        // A short queue: if decoding falls behind, the network side waits.
        let (units, mut receiver) = mpsc::channel::<Vec<u8>>(8);
        std::thread::Builder::new()
            .name("gantry-x1-decode".into())
            .spawn(move || decode_loop(&mut receiver, interval, quality, &jpegs))?;
        Ok(Self { units })
    }

    /// Queues an access unit. `false` once the worker has stopped.
    pub async fn send(&self, unit: Vec<u8>) -> bool {
        self.units.send(unit).await.is_ok()
    }
}

fn decode_loop(
    units: &mut mpsc::Receiver<Vec<u8>>,
    interval: Duration,
    quality: u8,
    jpegs: &mpsc::Sender<Bytes>,
) {
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "could not start the H.264 decoder");
            return;
        }
    };
    let mut last_sent: Option<Instant> = None;
    let mut rgb = Vec::new();
    while let Some(unit) = units.blocking_recv() {
        if jpegs.is_closed() {
            return;
        }
        let picture = match decoder.decode(&unit) {
            Ok(Some(picture)) => picture,
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!(error = %e, "skipped an undecodable H.264 frame");
                continue;
            }
        };
        if last_sent.is_some_and(|at| at.elapsed() < interval) {
            continue;
        }
        let (w, h) = picture.dimensions();
        let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else {
            continue;
        };
        rgb.resize(w * h * 3, 0);
        picture.write_rgb8(&mut rgb);
        let mut jpeg = Vec::with_capacity(w * h / 4);
        if jpeg_encoder::Encoder::new(&mut jpeg, quality)
            .encode(&rgb, w16, h16, jpeg_encoder::ColorType::Rgb)
            .is_err()
        {
            continue;
        }
        last_sent = Some(Instant::now());
        // A slow viewer misses frames rather than holding up decoding.
        match jpegs.try_send(Bytes::from(jpeg)) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp(sequence: u16, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, if marker { 0x80 | 96 } else { 96 }];
        p.extend_from_slice(&sequence.to_be_bytes());
        p.extend_from_slice(&[0; 8]);
        p.extend_from_slice(payload);
        p
    }

    const SPS: [u8; 3] = [0x67, 1, 2];
    const PPS: [u8; 2] = [0x68, 3];

    #[test]
    fn reassembles_fu_a_and_adds_parameter_sets_to_keyframes() {
        let mut d = Depacketizer::new(vec![SPS.to_vec(), PPS.to_vec()]);
        // An IDR slice (type 5, nri 3) split in three.
        assert!(d.push(&rtp(1, false, &[0x7C, 0x85, 0xAA])).is_none());
        assert!(d.push(&rtp(2, false, &[0x7C, 0x05, 0xBB])).is_none());
        let unit = d.push(&rtp(3, true, &[0x7C, 0x45, 0xCC])).unwrap();
        let mut expected = vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3];
        expected.extend_from_slice(&[0, 0, 0, 1, 0x65, 0xAA, 0xBB, 0xCC]);
        assert_eq!(unit, expected);
        // A P-slice after it passes on as it is.
        let unit = d.push(&rtp(4, true, &[0x41, 9])).unwrap();
        assert_eq!(unit, [0, 0, 0, 1, 0x41, 9]);
    }

    #[test]
    fn unpacks_stap_a() {
        let mut d = Depacketizer::new(vec![]);
        let mut stap = vec![0x78, 0, 3];
        stap.extend_from_slice(&SPS);
        stap.extend_from_slice(&[0, 2]);
        stap.extend_from_slice(&PPS);
        assert!(d.push(&rtp(10, false, &stap)).is_none());
        let unit = d.push(&rtp(11, true, &[0x65, 7])).unwrap();
        assert_eq!(
            unit,
            [
                0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 7
            ]
        );
    }

    #[test]
    fn after_a_lost_packet_waits_for_a_keyframe() {
        let mut d = Depacketizer::new(vec![SPS.to_vec(), PPS.to_vec()]);
        assert!(d.push(&rtp(1, true, &[0x65, 1])).is_some());
        // Packet 2 is lost.
        assert!(d.push(&rtp(3, true, &[0x41, 1])).is_none());
        assert!(d.push(&rtp(4, true, &[0x41, 2])).is_none());
        assert!(d.push(&rtp(5, true, &[0x65, 2])).is_some());
        // Nothing before the first keyframe either.
        let mut fresh = Depacketizer::new(vec![]);
        assert!(fresh.push(&rtp(1, true, &[0x41, 1])).is_none());
    }

    #[test]
    fn ignores_what_is_not_rtp() {
        let mut d = Depacketizer::new(vec![]);
        assert!(d.push(&[1, 2, 3]).is_none());
        assert!(d.push(&[0x40; 20]).is_none());
    }
}
