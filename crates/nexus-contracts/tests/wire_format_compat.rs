//! Cross-format MessagePack compatibility probe (ADR-213 R7 / W8 F1 decision).
//!
//! WHY THIS TEST EXISTS INSTEAD OF A DUAL-READ SHIM
//! -------------------------------------------------
//! ADR-213 R7 scheduled an "F1" wave: before switching writers from
//! `rmp_serde::to_vec` (positional array) to `to_vec_named` (named map), add a
//! reader that tries one form and falls back to the other. That plan assumed a
//! serde struct deserializer only accepts the representation its writer produced.
//! If that assumption is FALSE -- i.e. the derived `Deserialize` accepts both a
//! seq and a map -- then F1 is dead weight: the switch in F2 alone is backward and
//! forward compatible, and a hand-written compat shim would only add a path nobody
//! exercises.
//!
//! So this file measures the behaviour instead of assuming it in either direction.
//!
//! MEASURED OUTCOME (2026-10-13, this file): one `from_slice::<T>` accepts BOTH forms
//! for the types this crate persists, including when an Option field is skipped --
//! so **F1 (reader-side dual decode) is cancelled as unnecessary**, and the writer
//! switch (F2) is backward/forward compatible on its own. What named encoding buys is
//! `field_evolution_matrix_positional_vs_named` below: swapping two same-typed fields
//! silently swaps their VALUES in array form while the map form stays correct. The
//! file used to assert that arrays cannot gain a trailing field; measurement said
//! otherwise and the claim was removed -- see the two evolution tests.
//!
//! It is a permanent regression lock, not a scratch experiment: if a future serde /
//! rmp-serde upgrade changes the tolerance, this test goes red at the upgrade commit
//! rather than in production against persisted blobs.
//!
//! SCOPE NOTE
//! ----------
//! Only the two encodings actually in use in this repository are probed
//! (`to_vec`, `to_vec_named`). `to_named_vec` does not exist in rmp-serde and is
//! deliberately not referenced.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench/example idiom; E-5 targets production code

use nexus_contracts::{Checkpoint, SegmentCreationReason, SegmentMetadata, TokenLedgerEntry};

/// A checkpoint as quest-engine writes it today: `rmp_serde::to_vec` (array).
fn sample_checkpoint() -> Checkpoint {
    Checkpoint::new(
        "quest-1",
        "ckpt-1",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        vec![1u8, 2, 3, 4],
    )
}

/// A segment as the token-evidence ledger writes it.
fn sample_segment() -> SegmentMetadata {
    SegmentMetadata::new(
        "seg-1",
        "traj-parent-1",
        0,
        true,
        vec![],
        vec![1, 2, 3],
        0,
        4,
        SegmentCreationReason::TitoFallback,
    )
}

#[test]
fn legacy_positional_payload_still_decodes() {
    // Baseline: today's writer form must remain readable after any F2 change.
    let original = sample_checkpoint();
    let bytes = rmp_serde::to_vec(&original).expect("positional encode");
    let back: Checkpoint = rmp_serde::from_slice(&bytes).expect("positional decode");
    assert_eq!(original, back);
}

#[test]
fn named_payload_decodes_through_the_same_reader() {
    // The load-bearing question: does ONE `from_slice::<T>` call accept BOTH
    // representations? If this passes, F2 needs no reader-side shim (F1 skipped).
    let original = sample_checkpoint();
    let bytes = rmp_serde::to_vec_named(&original).expect("named encode");
    let back: Checkpoint = rmp_serde::from_slice(&bytes).expect("named decode via plain reader");
    assert_eq!(original, back);
}

#[test]
fn cross_form_read_works_when_an_option_field_is_skipped() {
    // Checkpoint's `description` carries `#[serde(default, skip_serializing_if)]`
    // (checkpoint.rs:48-49), so when it is None the MAP simply omits a key while the
    // ARRAY simply gets shorter. That is the exact place where "one reader, both
    // forms" could plausibly break, so it is tested rather than assumed.
    let mut bare = sample_checkpoint();
    bare.description = None;
    let mut described = sample_checkpoint();
    described.description = Some("resume point".to_string());

    for original in [&bare, &described] {
        let named = rmp_serde::to_vec_named(original).expect("named encode");
        let positional = rmp_serde::to_vec(original).expect("positional encode");
        assert_ne!(
            named, positional,
            "the two forms must differ, else the wave proves nothing"
        );
        let a: Checkpoint =
            rmp_serde::from_slice(&named).unwrap_or_else(|e| panic!("named decode failed: {e}"));
        let b: Checkpoint = rmp_serde::from_slice(&positional)
            .unwrap_or_else(|e| panic!("positional decode failed: {e}"));
        assert_eq!(&a, original);
        assert_eq!(&b, original);
    }

    // And the two forms must not be able to masquerade as each other's payloads.
    let named_bare = rmp_serde::to_vec_named(&bare).unwrap();
    let read: Checkpoint = rmp_serde::from_slice(&named_bare).unwrap();
    assert!(read.description.is_none());
}

#[test]
fn cross_form_read_works_for_a_type_holding_an_enum_and_a_byte_slice() {
    // SegmentMetadata carries a `SegmentCreationReason` (renamed in D-9) and a
    // `Box<[u8]>` snapshot, i.e. the shapes quest-engine/cmt-tiering actually persist.
    let original = sample_segment();
    let named = rmp_serde::to_vec_named(&original).expect("named encode");
    let positional = rmp_serde::to_vec(&original).expect("positional encode");
    assert_ne!(named, positional);
    assert_eq!(
        rmp_serde::from_slice::<SegmentMetadata>(&named).unwrap(),
        original
    );
    assert_eq!(
        rmp_serde::from_slice::<SegmentMetadata>(&positional).unwrap(),
        original
    );
}

/// Field-evolution matrix, MEASURED on rmp-serde (not assumed). This is the only
/// normative basis for switching writers to `to_vec_named` (ADR-213 R7, corrected
/// after measurement -- see docs/reports/L0-contracts-residual-debt_2026-10-13.md §7).
///
/// ```text
///   change                     named            positional(array)
///   ------------------------   ---------------  -----------------------------
///   append field at tail       Ok, new=0        Ok, new=0        <- BOTH safe
///   insert field in middle     Ok, b stays 2    Err(invalid len) <- loud, safe
///   swap two same-typed fields Ok, correct      Ok, VALUES SWAPPED  <- silent bug
/// ```
///
/// The silent row is the one that matters: it produces no error, no test failure at
/// the decode site, and corrupt data. Cross-form readability (the other tests here)
/// is why a reader-side dual-decode shim is NOT needed; this row is why the WRITER
/// must be named.
#[test]
fn field_evolution_matrix_positional_vs_named() {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V1 {
        a: u32,
        b: u32,
    }
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V2Tail {
        a: u32,
        b: u32,
        #[serde(default)]
        schema_version: u16,
    }
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V2Mid {
        a: u32,
        #[serde(default)]
        schema_version: u16,
        b: u32,
    }
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V2Swap {
        b: u32,
        a: u32,
    }

    let v1 = V1 { a: 1, b: 2 };
    let named = rmp_serde::to_vec_named(&v1).unwrap();
    let positional = rmp_serde::to_vec(&v1).unwrap();

    // Row 1 -- tail append: safe in BOTH forms, so `schema_version` may land before
    // or after the encoding switch without stranding old payloads.
    assert_eq!(
        rmp_serde::from_slice::<V2Tail>(&named).unwrap(),
        V2Tail {
            a: 1,
            b: 2,
            schema_version: 0
        }
    );
    assert_eq!(
        rmp_serde::from_slice::<V2Tail>(&positional).unwrap(),
        V2Tail {
            a: 1,
            b: 2,
            schema_version: 0
        }
    );

    // Row 2 -- middle insert: named keeps the right value per key; positional fails
    // LOUDLY on arity. A loud failure is acceptable, so this is not the argument.
    assert_eq!(
        rmp_serde::from_slice::<V2Mid>(&named).unwrap(),
        V2Mid {
            a: 1,
            schema_version: 0,
            b: 2
        }
    );
    assert!(rmp_serde::from_slice::<V2Mid>(&positional).is_err());

    // Row 3 -- the decisive case: named is correct, positional silently swaps.
    assert_eq!(
        rmp_serde::from_slice::<V2Swap>(&named).unwrap(),
        V2Swap { b: 2, a: 1 }
    );
    assert_eq!(
        rmp_serde::from_slice::<V2Swap>(&positional).unwrap(),
        V2Swap { b: 1, a: 2 },
        "documented hazard: array form re-binds by position"
    );
}

/// Byte-neighbourhood proof for F3: `Checkpoint` gained a tail `schema_version: u16`
/// that is SKIPPED while 0. A mirror of the pre-change shape is decoded/encoded here so
/// "adding the field changed no bytes" is measured, not asserted from serde folklore.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct CheckpointV0Mirror {
    quest_id: String,
    checkpoint_id: String,
    memory_snapshot_hash: String,
    serialized_state: Vec<u8>,
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[test]
fn adding_schema_version_changes_no_bytes_at_version_zero() {
    for described in [false, true] {
        let cp = if described {
            Checkpoint::with_description("q1", "c1", "hash", vec![1, 2, 3], "note")
        } else {
            Checkpoint::new("q1", "c1", "hash", vec![1, 2, 3])
        };
        let mirror = CheckpointV0Mirror {
            quest_id: cp.quest_id.clone(),
            checkpoint_id: cp.checkpoint_id.clone(),
            memory_snapshot_hash: cp.memory_snapshot_hash.clone(),
            serialized_state: cp.serialized_state.clone(),
            created_at: cp.created_at,
            description: cp.description.clone(),
        };
        assert_eq!(
            cp.schema_version, 0,
            "constructors must default to the legacy shape"
        );
        assert_eq!(
            rmp_serde::to_vec(&cp).unwrap(),
            rmp_serde::to_vec(&mirror).unwrap(),
            "array form must be byte-identical at version 0 (described={described})"
        );
        assert_eq!(
            rmp_serde::to_vec_named(&cp).unwrap(),
            rmp_serde::to_vec_named(&mirror).unwrap(),
            "named form must be byte-identical at version 0 (described={described})"
        );
        // And the pre-change payload still reads back into the post-change type.
        let old_bytes = rmp_serde::to_vec_named(&mirror).unwrap();
        let back: Checkpoint =
            rmp_serde::from_slice(&old_bytes).expect("old payload must still decode");
        assert_eq!(back, cp);
    }
}

#[test]
fn forward_compat_needs_named_form_measured_not_assumed() {
    // Refines an earlier claim in this file's header: F2 is not needed for BACKWARD
    // compat (both forms read each other), but it IS what buys FORWARD compat -- an
    // old reader can skip an unknown KEY, while an array reader cannot identify an
    // unexpected extra trailing element. Measured both ways rather than asserted.
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Old {
        a: u32,
        b: u32,
    }
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Newer {
        a: u32,
        b: u32,
        #[serde(default)]
        schema_version: u16,
    }

    let new = Newer {
        a: 1,
        b: 2,
        schema_version: 1,
    };
    let named = rmp_serde::to_vec_named(&new).unwrap();
    let array = rmp_serde::to_vec(&new).unwrap();

    let via_named = rmp_serde::from_slice::<Old>(&named);
    let via_array = rmp_serde::from_slice::<Old>(&array);
    println!(
        "forward-compat: old<-named {:?} | old<-array {:?}",
        via_named.is_ok(),
        via_array.is_ok()
    );
    assert!(
        via_named.is_ok(),
        "named form must let an older reader ignore new keys"
    );
    assert!(
        via_array.is_err(),
        "array form cannot express an added field to an old reader -- \
                                 this is the measured reason Class A envelopes must be named"
    );
}

/// Mirror of the pre-F3 `SegmentMetadata` shape (tail `schema_version` absent), used to
/// prove byte-neutrality of the added field on the type whose sibling in the same file
/// carries a documented array-encoding trap (`graph_identity` skips WITHOUT default).
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct SegmentV0Mirror {
    segment_id: Box<str>,
    parent_traj_id: Box<str>,
    segment_index: u32,
    is_anchor: bool,
    token_entries: Vec<Box<str>>,
    context_snapshot: Box<[u8]>,
    start_turn: u32,
    end_turn: u32,
    creation_reason: SegmentCreationReason,
}

#[test]
fn segment_metadata_schema_version_changes_no_bytes() {
    let seg = sample_segment();
    let mirror = SegmentV0Mirror {
        segment_id: seg.segment_id.clone(),
        parent_traj_id: seg.parent_traj_id.clone(),
        segment_index: seg.segment_index,
        is_anchor: seg.is_anchor,
        token_entries: seg.token_entries.clone(),
        context_snapshot: seg.context_snapshot.clone(),
        start_turn: seg.start_turn,
        end_turn: seg.end_turn,
        creation_reason: seg.creation_reason,
    };
    assert_eq!(seg.schema_version, 0);
    assert_eq!(
        rmp_serde::to_vec(&seg).unwrap(),
        rmp_serde::to_vec(&mirror).unwrap(),
        "array bytes must be unchanged at version 0"
    );
    assert_eq!(
        rmp_serde::to_vec_named(&seg).unwrap(),
        rmp_serde::to_vec_named(&mirror).unwrap(),
        "named bytes must be unchanged at version 0"
    );
    let old_named = rmp_serde::to_vec_named(&mirror).unwrap();
    let old_array = rmp_serde::to_vec(&mirror).unwrap();
    assert_eq!(
        rmp_serde::from_slice::<SegmentMetadata>(&old_named).unwrap(),
        seg
    );
    assert_eq!(
        rmp_serde::from_slice::<SegmentMetadata>(&old_array).unwrap(),
        seg
    );
}

#[test]
fn token_ledger_entry_round_trips_in_both_forms_after_f3() {
    // TokenLedgerEntry has 13 fields and a documented skip-without-default trap on
    // `graph_identity`; the new field pairs default+skip, so both encodings must
    // round-trip and a v0 payload must read back unchanged.
    let entry = TokenLedgerEntry::new(
        "te-1",
        7,
        "sess-1",
        "inst-1",
        vec![1u32, 2, 3],
        vec![4u32, 5],
        vec![0.1f32, 0.2],
        vec![true, false],
        "v1",
        vec![],
        None,
        1_700_000_000,
    );
    assert_eq!(entry.schema_version, 0);
    for bytes in [
        rmp_serde::to_vec(&entry).unwrap(),
        rmp_serde::to_vec_named(&entry).unwrap(),
    ] {
        let back: TokenLedgerEntry = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(back, entry);
    }
}

#[test]
fn nonzero_schema_version_needs_named_form_and_array_form_is_measured_unsafe() {
    // Completes the version story, and the outcome is NOT what one would hope for:
    //  * named form: a non-zero version is written and survives a round-trip.
    //  * array form: because an EARLIER field is skippable (`description` is None +
    //    skip_serializing_if), a non-zero tail field lands in the WRONG position and is
    //    decoded against the skipped field's type -- here it fails loudly ("invalid
    //    type: integer `1`, expected a string"); with two same-typed neighbours it would
    //    silently mis-assign instead.
    // So in array form `schema_version` is only safe while it stays 0. That makes F3
    // gated on F2 by a second mechanism beyond forward-compatibility: positional
    // envelopes cannot carry versioned optional fields at all (ADR-213 R8.1).
    let mut versioned = sample_checkpoint();
    versioned.schema_version = 1;
    let plain = sample_checkpoint();

    let named = rmp_serde::to_vec_named(&versioned).unwrap();
    assert_ne!(
        named,
        rmp_serde::to_vec_named(&plain).unwrap(),
        "a non-zero version must actually be encoded, not dropped"
    );
    assert_eq!(
        rmp_serde::from_slice::<Checkpoint>(&named).unwrap(),
        versioned,
        "named form must carry the version"
    );

    let array = rmp_serde::to_vec(&versioned).unwrap();
    assert!(
        rmp_serde::from_slice::<Checkpoint>(&array).is_err(),
        "array form mis-binds a non-zero tail version when an earlier field is \
         skippable -- measured reason Class A envelopes must be named"
    );
}

#[test]
fn absent_field_is_tolerated_in_both_forms_when_appended_at_the_tail() {
    // Correction of the claim this file originally set out to prove. A field added at
    // the END with #[serde(default)] reads back fine from a positional payload too --
    // serde fills the missing trailing element. The forward-compatibility argument for
    // named encoding therefore rests on the SWAP row above, not on "arrays cannot grow".
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V1 {
        a: u32,
        b: u32,
    }
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct V2 {
        a: u32,
        b: u32,
        #[serde(default)]
        schema_version: u16,
    }

    let v1 = V1 { a: 1, b: 2 };
    let as_v2_named: V2 = rmp_serde::from_slice(&rmp_serde::to_vec_named(&v1).unwrap()).unwrap();
    let as_v2_array: V2 = rmp_serde::from_slice(&rmp_serde::to_vec(&v1).unwrap()).unwrap();
    assert_eq!(as_v2_named, as_v2_array);
    assert_eq!(as_v2_named.schema_version, 0);
}

#[test]
fn measure_named_vs_positional_payload_size() {
    // ADR-213 R7 performance face: the named form repeats each field name per record,
    // so it must cost bytes. Report the real numbers instead of asserting a made-up
    // threshold -- a threshold nobody chose is a fake gate. The values here are what
    // docs/reports/L0-contracts-residual-debt_2026-10-13.md §7 quotes.
    let cp = sample_checkpoint();
    let seg = sample_segment();
    for (label, bytes_named, bytes_array) in [
        ("Checkpoint", size_of_named(&cp), size_of_array(&cp)),
        ("SegmentMetadata", size_of_named(&seg), size_of_array(&seg)),
    ] {
        let delta = bytes_named as i64 - bytes_array as i64;
        println!(
            "size[{label}]: named={bytes_named}B array={bytes_array}B delta={delta:+}B \
             (x{} for a 1000-record WAL segment)",
            delta.max(0) * 1000
        );
        assert!(bytes_named > 0 && bytes_array > 0);
    }
}

fn size_of_named<T: serde::Serialize>(v: &T) -> usize {
    rmp_serde::to_vec_named(v).expect("named encode").len()
}

fn size_of_array<T: serde::Serialize>(v: &T) -> usize {
    rmp_serde::to_vec(v).expect("array encode").len()
}

#[test]
fn wire_string_for_renamed_variant_is_pinned() {
    // D-9 (ADR-213): the Rust identifier became `TitoFallback`, the on-wire string
    // had to stay `titofallback`. Pinned here so a future `rename_all` tweak cannot
    // silently migrate persisted payloads.
    let json = serde_json::to_string(&SegmentCreationReason::TitoFallback).unwrap();
    assert_eq!(json, "\"titofallback\"");
    let back: SegmentCreationReason = serde_json::from_str("\"titofallback\"").unwrap();
    assert_eq!(back, SegmentCreationReason::TitoFallback);
}
