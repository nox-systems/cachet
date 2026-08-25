//! Multipart part planning (CLAUDE.md §4, docs/testing/kani.md). The
//! upload protocol declares the total byte count up front, and every part
//! except the last carries exactly [`UPLOAD_PART_BYTES`]; the server
//! verifies completeness against this plan, so client and worker compute
//! it from one function.

use crate::constants::{COMPLETE_BODY_BYTES_MAX, MULTIPART_PARTS_MAX, UPLOAD_PART_BYTES};
use crate::error::{ClientError, Result};
use crate::upload_record::UploadRecord;

/// One planned part: a one-based part number and its exact byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    /// One-based part number, as the wire protocol carries it.
    pub number: u64,
    /// The exact byte length of this part.
    pub len: u64,
}

/// The full plan for one upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPlan {
    /// The declared total byte count of the upload.
    pub total_bytes: u64,
    /// The parts, in ascending order.
    pub parts: Vec<Part>,
}

/// The arithmetic shape of a plan: how many parts, and how long the last
/// one is. Kani proves every law of the plan over this shape alone, so the
/// materializing loop below never carries a symbolic bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanShape {
    /// The part count, between 1 and [`MULTIPART_PARTS_MAX`].
    pub count: u64,
    /// The byte length of the final part, between 1 and
    /// [`UPLOAD_PART_BYTES`].
    pub last_len: u64,
}

/// Compute the shape of the plan for an object of `total_bytes`.
///
/// # Errors
///
/// - [`ClientError::LengthRequired`] when the total is zero: the protocol
///   requires a declared positive total before any part is accepted.
/// - [`ClientError::BodyTooLarge`] when the total exceeds
///   [`UPLOAD_PART_BYTES`] times [`MULTIPART_PARTS_MAX`].
pub fn plan_shape(total_bytes: u64) -> Result<PlanShape> {
    if total_bytes == 0 {
        return Err(ClientError::LengthRequired);
    }
    if total_bytes > UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX {
        return Err(ClientError::BodyTooLarge);
    }
    let full_parts = total_bytes / UPLOAD_PART_BYTES;
    let remainder = total_bytes % UPLOAD_PART_BYTES;
    Ok(PlanShape {
        count: if remainder == 0 {
            full_parts
        } else {
            full_parts + 1
        },
        last_len: if remainder == 0 {
            UPLOAD_PART_BYTES
        } else {
            remainder
        },
    })
}

/// Materialize the plan for an object of `total_bytes`, one part per part
/// number in ascending order.
///
/// # Errors
///
/// The same failures as [`plan_shape`].
pub fn part_plan(total_bytes: u64) -> Result<PartPlan> {
    let shape = plan_shape(total_bytes)?;
    let mut parts = Vec::with_capacity(usize::try_from(shape.count).expect("count ≤ 1000"));
    for n in 1..=shape.count {
        let len = if n == shape.count {
            shape.last_len
        } else {
            UPLOAD_PART_BYTES
        };
        parts.push(Part { number: n, len });
    }
    debug_assert_eq!(parts.iter().map(|p| p.len).sum::<u64>(), total_bytes);
    Ok(PartPlan { total_bytes, parts })
}

/// The exact size one part must carry, given the plan totals.
#[must_use]
pub fn expected_part_bytes(total_bytes: u64, expected_parts: u64, part_number: u64) -> u64 {
    debug_assert!(part_number >= 1 && part_number <= expected_parts);
    if part_number == expected_parts {
        total_bytes - (expected_parts - 1) * UPLOAD_PART_BYTES
    } else {
        UPLOAD_PART_BYTES
    }
}

/// Validate one part before it is stored. The check runs here, per
/// request, because R2's own uniform-part rule fires only at completion —
/// after every byte has crossed the network. Catching a wrong-sized part
/// on arrival costs the client one request, where catching it at
/// completion costs the whole upload.
///
/// # Errors
///
/// [`ClientError::PartNumberInvalid`] outside `1..=record.expected_parts`;
/// [`ClientError::PartSizeMismatch`] when the declared length is not the
/// planned size for this part.
pub fn check_part(record: &UploadRecord, part_number: u64, content_length: u64) -> Result<()> {
    if part_number == 0 || part_number > record.expected_parts {
        return Err(ClientError::PartNumberInvalid);
    }
    let expected = expected_part_bytes(record.total_bytes, record.expected_parts, part_number);
    if content_length != expected {
        return Err(ClientError::PartSizeMismatch);
    }
    Ok(())
}

/// One completed part, as the client reports it in the completion body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPart {
    /// The one-based part number.
    pub number: u64,
    /// The checksum R2 returned for the part when it was stored.
    pub etag: String,
}

/// Validate the part list a client sends to complete an upload. Exactly
/// the planned parts, once each, in ascending order: both what R2 expects
/// and the only list that can reassemble the object the client declared.
///
/// # Errors
///
/// [`ClientError::BodyTooLarge`] when the body exceeds
/// [`COMPLETE_BODY_BYTES_MAX`]; [`ClientError::CompletePartsMismatch`]
/// on any shape violation.
pub fn parse_completion_body(text: &str, record: &UploadRecord) -> Result<Vec<CompletedPart>> {
    if u64::try_from(text.len()).expect("len fits") > COMPLETE_BODY_BYTES_MAX {
        return Err(ClientError::BodyTooLarge);
    }
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|_| ClientError::CompletePartsMismatch)?;
    let entries = parsed
        .as_array()
        .ok_or(ClientError::CompletePartsMismatch)?;
    if u64::try_from(entries.len()).expect("len fits") != record.expected_parts {
        return Err(ClientError::CompletePartsMismatch);
    }
    let mut parts = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let number = entry
            .get("partNumber")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ClientError::CompletePartsMismatch)?;
        let etag = entry
            .get("etag")
            .and_then(serde_json::Value::as_str)
            .filter(|etag| !etag.is_empty())
            .ok_or(ClientError::CompletePartsMismatch)?;
        // Ascending and gapless, which also rules out duplicates without a
        // second pass.
        if number != u64::try_from(index).expect("index fits") + 1 {
            return Err(ClientError::CompletePartsMismatch);
        }
        parts.push(CompletedPart {
            number,
            etag: etag.to_string(),
        });
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_total_is_refused() {
        assert_eq!(plan_shape(0), Err(ClientError::LengthRequired));
    }

    #[test]
    fn over_the_cap_is_refused() {
        assert_eq!(
            plan_shape(UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX + 1),
            Err(ClientError::BodyTooLarge)
        );
    }

    #[test]
    fn exact_multiple_has_no_short_tail() {
        let plan = part_plan(2 * UPLOAD_PART_BYTES).expect("two full parts");
        assert_eq!(plan.parts.len(), 2);
        assert!(plan.parts.iter().all(|p| p.len == UPLOAD_PART_BYTES));
    }

    #[test]
    fn single_part_is_short() {
        let plan = part_plan(7).expect("one short part");
        assert_eq!(plan.parts.len(), 1);
        assert_eq!(plan.parts[0].len, 7);
    }

    #[test]
    fn shape_and_materialization_agree() {
        let shape = plan_shape(UPLOAD_PART_BYTES + 5).expect("small plan");
        let plan = part_plan(UPLOAD_PART_BYTES + 5).expect("small plan");
        assert_eq!(shape.count, plan.parts.len() as u64);
        assert_eq!(shape.last_len, plan.parts.last().expect("a last part").len);
    }

    fn record(total_bytes: u64, expected_parts: u64) -> UploadRecord {
        UploadRecord {
            key: format!("nar/{}.nar.zst", "x".repeat(52)),
            total_bytes,
            expected_parts,
            nar_bytes: total_bytes * 3,
            created_at_ms: 1,
        }
    }

    #[test]
    fn parts_check_against_the_plan() {
        let three = record(2 * UPLOAD_PART_BYTES + 7, 3);
        assert!(check_part(&three, 1, UPLOAD_PART_BYTES).is_ok());
        assert!(check_part(&three, 2, UPLOAD_PART_BYTES).is_ok());
        assert!(check_part(&three, 3, 7).is_ok());
        assert_eq!(
            check_part(&three, 3, UPLOAD_PART_BYTES),
            Err(ClientError::PartSizeMismatch)
        );
        assert_eq!(check_part(&three, 1, 7), Err(ClientError::PartSizeMismatch));
        assert_eq!(
            check_part(&three, 0, UPLOAD_PART_BYTES),
            Err(ClientError::PartNumberInvalid)
        );
        assert_eq!(
            check_part(&three, 4, 7),
            Err(ClientError::PartNumberInvalid)
        );
    }

    #[test]
    fn completions_require_the_whole_plan_in_order() {
        let three = record(2 * UPLOAD_PART_BYTES + 7, 3);
        let good = serde_json::json!([
            {"partNumber": 1, "etag": "aaa"},
            {"partNumber": 2, "etag": "bbb"},
            {"partNumber": 3, "etag": "ccc"},
        ]);
        let parts = parse_completion_body(&good.to_string(), &three).expect("the plan completes");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].etag, "ccc");

        for bad in [
            serde_json::json!({"not": "an array"}),
            serde_json::json!([{"partNumber": 1, "etag": "aaa"}]),
            serde_json::json!([
                {"partNumber": 1, "etag": "aaa"},
                {"partNumber": 1, "etag": "bbb"},
                {"partNumber": 3, "etag": "ccc"},
            ]),
            serde_json::json!([
                {"partNumber": 1, "etag": "aaa"},
                {"partNumber": 3, "etag": "bbb"},
                {"partNumber": 2, "etag": "ccc"},
            ]),
            serde_json::json!([
                {"partNumber": 1, "etag": ""},
                {"partNumber": 2, "etag": "bbb"},
                {"partNumber": 3, "etag": "ccc"},
            ]),
            serde_json::json!([
                {"partNumber": 1, "etag": "aaa"},
                {"partNumber": 2, "etag": "bbb"},
                {"partNumber": "3", "etag": "ccc"},
            ]),
        ] {
            assert_eq!(
                parse_completion_body(&bad.to_string(), &three),
                Err(ClientError::CompletePartsMismatch),
                "{bad}"
            );
        }
        assert_eq!(
            parse_completion_body("not json", &three),
            Err(ClientError::CompletePartsMismatch)
        );
        assert_eq!(
            parse_completion_body(&"[".repeat(300_000), &three),
            Err(ClientError::BodyTooLarge)
        );
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// For every admissible total: the shape's arithmetic sums exactly, the
    /// final part is positive, and the count stays inside the cap. For
    /// every inadmissible total: the refusal is typed. The proof runs over
    /// the shape so nothing allocates and no loop carries a symbolic bound.
    #[kani::proof]
    #[kani::unwind(1)]
    fn plan_shape_sums_and_bounds() {
        let total: u64 = kani::any();
        match plan_shape(total) {
            Ok(shape) => {
                assert!(
                    total > 0 && total <= UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX,
                    "success implies an admissible total"
                );
                assert_eq!(
                    (shape.count - 1) * UPLOAD_PART_BYTES + shape.last_len,
                    total,
                    "the shape sums to the declared total"
                );
                assert!(shape.count >= 1 && shape.count <= MULTIPART_PARTS_MAX);
                assert!(shape.last_len >= 1 && shape.last_len <= UPLOAD_PART_BYTES);
            }
            Err(code) => {
                assert!(
                    total == 0 || total > UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX,
                    "refusal implies an inadmissible total"
                );
                assert!(matches!(
                    code,
                    ClientError::LengthRequired | ClientError::BodyTooLarge
                ));
            }
        }
    }
}
