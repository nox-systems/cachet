//! Multipart part planning (CLAUDE.md §4, docs/testing/kani.md). The
//! upload protocol declares the total byte count up front, and every part
//! except the last carries exactly [`UPLOAD_PART_BYTES`]; the server
//! verifies completeness against this plan, so client and worker compute
//! it from one function.

use crate::constants::{MULTIPART_PARTS_MAX, UPLOAD_PART_BYTES};
use crate::error::{ClientError, Result};

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
