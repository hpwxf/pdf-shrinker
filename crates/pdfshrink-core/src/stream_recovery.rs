//! Streams `lopdf` loads empty because their dictionary has no usable `/Length`
//! (D-026).
//!
//! When `/Length` is absent (some producers write only `/Length1` on a
//! `/FontFile2`), lopdf's parser defers the stream (`Stream::with_position`),
//! then fails to read it for want of a length and ignores the error: the stream
//! stays empty. A lost font program makes viewers substitute a generic font, a
//! lost content stream blanks a page — at every level, `lossless` included.
//!
//! [`recover_missing_lengths`] reads such streams back from the input bytes with
//! the rule lopdf itself applies to a *wrong* `/Length` (`recover_stream_length`,
//! lopdf ≥ 0.45): the payload ends at the only EOL-framed `endstream` followed
//! by `endobj`, without crossing into the next object of the cross-reference
//! table. [`first_unreadable_stream`] finds what is still lost after that, so
//! the engine refuses the file instead of writing a degraded copy (D-005).
//!
//! Remove this module once lopdf recovers a missing `/Length` on load.

use lopdf::xref::XrefEntry;
use lopdf::{Document, Object, ObjectId, Stream};

/// Reads back every stream lopdf left empty for want of a `/Length`, setting
/// its content (and so its `/Length`). Returns how many were recovered.
pub(crate) fn recover_missing_lengths(doc: &mut Document, input: &[u8]) -> usize {
    let mut offsets: Vec<usize> = doc
        .reference_table
        .entries
        .values()
        .filter_map(|entry| match entry {
            XrefEntry::Normal { offset, .. } => Some(*offset as usize),
            _ => None,
        })
        .collect();
    offsets.sort_unstable();

    let lost: Vec<(ObjectId, usize)> = doc
        .objects
        .iter()
        .filter_map(|(id, obj)| match obj {
            Object::Stream(s) if is_unread(doc, s) => Some((*id, s.start_position?)),
            _ => None,
        })
        .collect();

    let mut recovered = 0;
    for (id, start) in lost {
        if start > input.len() {
            continue;
        }
        // The next object in the file bounds the scan: the payload cannot run into it.
        let bound = offsets
            .iter()
            .copied()
            .find(|&offset| offset > start)
            .map_or(input.len(), |offset| offset.min(input.len()));
        let Some(len) = payload_len(&input[start..bound]) else {
            continue;
        };
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            s.set_content(input[start..start + len].to_vec());
            recovered += 1;
        }
    }
    recovered
}

/// The first stream still lost after [`recover_missing_lengths`]: deferred by
/// the parser, empty, and without a `/Length` that resolves to a number.
pub(crate) fn first_unreadable_stream(doc: &Document) -> Option<ObjectId> {
    doc.objects.iter().find_map(|(id, obj)| match obj {
        Object::Stream(s) if is_unread(doc, s) => Some(*id),
        _ => None,
    })
}

fn is_unread(doc: &Document, s: &Stream) -> bool {
    s.start_position.is_some() && s.content.is_empty() && !has_length(doc, s)
}

fn has_length(doc: &Document, s: &Stream) -> bool {
    s.dict
        .get(b"Length")
        .and_then(|v| doc.dereference(v))
        .is_ok_and(|(_, length)| length.as_i64().is_ok() || length.as_f32().is_ok())
}

/// Length of the payload at the start of `data` (which begins right after the
/// `stream` keyword's EOL): up to the EOL before the only `endstream` that is
/// followed by white-space, `endobj`, then white-space or the end of `data`.
/// `None` when there is no such `endstream`, or more than one.
fn payload_len(data: &[u8]) -> Option<usize> {
    const ENDSTREAM: &[u8] = b"endstream";
    const ENDOBJ: &[u8] = b"endobj";
    let mut found = None;
    for pos in 0..data.len().saturating_sub(ENDSTREAM.len() - 1) {
        if !data[pos..].starts_with(ENDSTREAM) {
            continue;
        }
        let end = if pos == 0 {
            0 // empty payload: the EOL after `stream` was the only one
        } else if data[..pos].ends_with(b"\r\n") {
            pos - 2
        } else if data[..pos].ends_with(b"\n") || data[..pos].ends_with(b"\r") {
            pos - 1
        } else {
            continue;
        };
        let mut after = pos + ENDSTREAM.len();
        while data.get(after).is_some_and(|&b| is_whitespace(b)) {
            after += 1;
        }
        if !data[after..].starts_with(ENDOBJ) {
            continue;
        }
        if data
            .get(after + ENDOBJ.len())
            .is_some_and(|&b| !is_whitespace(b))
        {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(end);
    }
    found
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

#[cfg(test)]
mod tests {
    use super::payload_len;

    #[test]
    fn payload_ends_at_the_eol_before_endstream_endobj() {
        assert_eq!(payload_len(b"abc\nendstream\nendobj\n"), Some(3));
        assert_eq!(payload_len(b"abc\r\nendstream endobj"), Some(3));
        assert_eq!(payload_len(b"endstream\nendobj\n"), Some(0));
    }

    #[test]
    fn endstream_inside_the_payload_is_skipped() {
        // Not followed by `endobj`, or not EOL-framed: part of the data.
        let data = b"a\nendstream x\nb endstream\nendobj\nc\nendstream\nendobj";
        assert_eq!(
            payload_len(data),
            Some(data.len() - b"\nendstream\nendobj".len())
        );
    }

    #[test]
    fn ambiguous_or_missing_end_is_not_recovered() {
        assert_eq!(
            payload_len(b"a\nendstream\nendobj\nb\nendstream\nendobj\n"),
            None
        );
        assert_eq!(payload_len(b"abc\nendobj\n"), None);
        assert_eq!(payload_len(b"abc\nendstream\nendobjx"), None);
    }
}
