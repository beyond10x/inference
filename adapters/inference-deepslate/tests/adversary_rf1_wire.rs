//! Adversary cases: the hand-written protocol buffers codec, through the public `protocol`
//! messages, against the encoding guide and the published `realtime.proto` field numbers.
//! Varint edges, unknown fields of every wire type, packed repeated fields, lengths past the end
//! or absurdly large, and the documented `STRUCT_DEPTH` bound. Every case runs under a deadline.

mod support;

use inference_deepslate::protocol::{
    ClientBound, ErrorCategory, ProtocolError, STRUCT_DEPTH, ServiceBound, TriggerMode, VadState,
};
use serde_json::{Value as Json, json};
use support::within_deadline;

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn len_field(field: u32, body: &[u8], out: &mut Vec<u8>) {
    varint(u64::from(field) << 3 | 2, out);
    varint(body.len() as u64, out);
    out.extend_from_slice(body);
}

#[test]
fn the_largest_packet_id_is_a_ten_byte_varint_and_reads_back() {
    within_deadline(|| {
        let message = ServiceBound::Audio {
            packet_id: u64::MAX,
            mode: TriggerMode::Immediate,
            data: vec![1, 2],
        };
        let bytes = message.encode();
        assert_eq!(
            &bytes[2..13],
            &[
                0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01
            ]
        );
        assert_eq!(ServiceBound::decode(&bytes), Ok(message));

        let vad = ClientBound::VadStateEvent {
            from: VadState::Speech,
            to: VadState::Silence,
            packet_id: u64::MAX,
        };
        assert_eq!(ClientBound::decode(&vad.encode()), Ok(vad));
    });
}

#[test]
fn a_negative_enum_is_ten_bytes_and_reads_as_an_unnamed_category() {
    within_deadline(|| {
        let mut body = vec![0x08];
        varint((-1_i64) as u64, &mut body);
        assert_eq!(body.len(), 11);
        let mut frame = Vec::new();
        len_field(8, &body, &mut frame);
        assert_eq!(
            ClientBound::decode(&frame),
            Ok(ClientBound::Error {
                category: ErrorCategory::Other(-1),
                message: String::new(),
                trace_id: None,
            })
        );
    });
}

#[test]
fn unknown_fields_of_every_wire_type_are_skipped_inside_and_outside_a_member() {
    within_deadline(|| {
        let mut inner = Vec::new();
        inner.extend_from_slice(&[0x10, 0xff, 0xff, 0xff, 0xff, 0x0f]);
        inner.extend_from_slice(&[0x19, 1, 2, 3, 4, 5, 6, 7, 8]);
        len_field(4, &[0x0b, 0xff, 0x80], &mut inner);
        inner.extend_from_slice(&[0x2d, 9, 9, 9, 9]);
        inner.extend_from_slice(&[0x08, 0x07]);
        len_field(9, &[], &mut inner);
        let mut frame = Vec::new();
        frame.extend_from_slice(&[0xf8, 0xff, 0xff, 0xff, 0x0f, 0x2a]);
        len_field(5, &inner, &mut frame);
        len_field(99, &[0xde, 0xad], &mut frame);
        assert_eq!(
            ClientBound::decode(&frame),
            Ok(ClientBound::ResponseBegin { turn_id: 7 })
        );
    });
}

#[test]
fn a_packed_repeated_member_the_binding_passes_over_is_other() {
    within_deadline(|| {
        let mut packed = Vec::new();
        for turn in [1_u64, 2, 300, u64::from(u32::MAX)] {
            varint(turn, &mut packed);
        }
        let mut truncated = Vec::new();
        len_field(1, &packed, &mut truncated);
        truncated.extend_from_slice(&[0x10, 0x09]);
        let mut frame = Vec::new();
        len_field(14, &truncated, &mut frame);
        assert_eq!(ClientBound::decode(&frame), Ok(ClientBound::Other(14)));
    });
}

#[test]
fn an_overlong_varint_of_a_small_value_reads_as_that_value() {
    within_deadline(|| {
        assert_eq!(
            ClientBound::decode(&[0x2a, 0x05, 0x08, 0x84, 0x80, 0x80, 0x00]),
            Ok(ClientBound::ResponseBegin { turn_id: 4 })
        );
    });
}

#[test]
fn varints_past_sixty_four_bits_or_ten_bytes_are_refused() {
    within_deadline(|| {
        let eleven = [
            0x2a, 0x0c, 0x08, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00,
        ];
        assert_eq!(ClientBound::decode(&eleven), Err(ProtocolError::Varint));
        let too_big = [
            0x68, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02,
        ];
        assert_eq!(ClientBound::decode(&too_big), Err(ProtocolError::Varint));
    });
}

#[test]
fn lengths_past_the_end_or_absurd_are_refused_without_allocating() {
    within_deadline(|| {
        assert_eq!(
            ClientBound::decode(&[0x2a, 0x05, 0x08]),
            Err(ProtocolError::Truncated)
        );
        let mut huge = vec![0x1a];
        varint(u64::MAX, &mut huge);
        huge.push(0x0a);
        assert_eq!(ClientBound::decode(&huge), Err(ProtocolError::Truncated));
        let mut nested = Vec::new();
        let mut chunk = vec![0x0a];
        varint(1 << 62, &mut chunk);
        chunk.extend_from_slice(&[0x0a, 0x00]);
        len_field(3, &chunk, &mut nested);
        assert_eq!(ClientBound::decode(&nested), Err(ProtocolError::Truncated));
        let mut user = vec![0x1a];
        varint(1 << 40, &mut user);
        assert_eq!(ServiceBound::decode(&user), Err(ProtocolError::Truncated));
    });
}

#[test]
fn a_zero_length_audio_chunk_and_empty_members_read_as_empty() {
    within_deadline(|| {
        assert_eq!(
            ClientBound::decode(&[0x1a, 0x00]),
            Ok(ClientBound::ModelAudioChunk {
                audio: Vec::new(),
                turn_id: None,
            })
        );
        assert_eq!(
            ClientBound::decode(&[0x1a, 0x04, 0x0a, 0x00, 0x18, 0x00]),
            Ok(ClientBound::ModelAudioChunk {
                audio: Vec::new(),
                turn_id: Some(0),
            })
        );
        assert_eq!(
            ClientBound::decode(&[0x32, 0x00]),
            Ok(ClientBound::ResponseEnd { turn_id: 0 })
        );
    });
}

/// `STRUCT_DEPTH` says how deep a `google.protobuf.Struct` may nest before it is refused. Tool
/// parameters nested 20 objects deep, well inside 32, must be read. The reader counts a
/// `Struct` and the `Value` holding it as two levels, so the real bound is 16 objects.
#[test]
fn tool_parameters_nested_within_struct_depth_are_read() {
    within_deadline(|| {
        let mut value = json!("leaf");
        for _ in 0..20 {
            value = json!({ "a": value });
        }
        let call = ClientBound::ToolCallRequest {
            id: "c".to_owned(),
            name: "t".to_owned(),
            parameters: value,
            turn_id: Some(1),
        };
        assert_eq!(
            ClientBound::decode(&call.encode()),
            Ok(call),
            "an object nested 20 deep is refused although STRUCT_DEPTH is {STRUCT_DEPTH}"
        );
    });
}

/// A list nests one level per list, so 30 nested lists are read: the bound counts lists and
/// objects differently.
#[test]
fn lists_nested_within_struct_depth_are_read() {
    within_deadline(|| {
        let mut value = json!("leaf");
        for _ in 0..30 {
            value = Json::Array(vec![value]);
        }
        let call = ClientBound::ToolCallRequest {
            id: "c".to_owned(),
            name: "t".to_owned(),
            parameters: json!({ "a": value }),
            turn_id: None,
        };
        assert_eq!(ClientBound::decode(&call.encode()), Ok(call));
    });
}

/// Arbitrary bytes, from a fixed-seed generator, never panic either reader.
#[test]
fn arbitrary_bytes_never_panic_a_reader() {
    within_deadline(|| {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let length = (next() % 48) as usize;
            let bytes: Vec<u8> = (0..length).map(|_| next() as u8).collect();
            let _ = ClientBound::decode(&bytes);
            let _ = ServiceBound::decode(&bytes);
            let mut framed = Vec::new();
            for field in [1_u32, 3, 4, 5, 8, 13] {
                framed.clear();
                len_field(field, &bytes, &mut framed);
                let _ = ClientBound::decode(&framed);
                let _ = ServiceBound::decode(&framed);
            }
        }
    });
}
