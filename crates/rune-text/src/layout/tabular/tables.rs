//! Fail-closed OpenType table safety verification for tabular-digit substitution.
//!
//! Shaping the 10 uniform fills (`'0'..='9'`) and an order-2 De Bruijn sequence over `'0'..='9'`
//! exhaustively tests every single glyph and every adjacent pair (including `GPOS` PairPos and
//! legacy Format-0 `kern`). This module closes the remaining multi-glyph and contextual cases:
//! ligatures across mixed digits and separators (e.g. `1/2` under `frac` or `dlig`), contextual
//! GSUB/GPOS rules of length >= 3, mark/cursive attachments, and AAT state-machine tables
//! (`morx`/`mort`/`kerx`). Any table or rule that cannot be proven invariant to digit choice
//! returns `false` so the caller falls back to full shaping.

use rune_core::tok::{Family, TextStyle};
use skrifa::raw::tables::gpos::{self, PositionLookup, PositionSubtables};
use skrifa::raw::tables::gsub::{self, SingleSubst, SubstitutionLookup, SubstitutionSubtables};
use skrifa::raw::tables::layout::{
    ChainedSequenceContext, ClassDef, CoverageTable, FeatureList, FeatureVariations, LookupFlag,
    SequenceContext, SequenceLookupRecord,
};
use skrifa::raw::{FontRef, TableProvider};
use skrifa::{GlyphId, GlyphId16, MetadataProvider, Tag};

/// Per-position reachable glyph sets inside one shaped run.
struct RunState {
    /// For each byte position in the run: `Some(slot_idx)` if a digit slot, else `None`.
    digit_slot: Vec<Option<usize>>,
    /// For each position in the run: all glyph IDs reachable so far (over-approximation used to
    /// detect whether any ligature or contextual rule can possibly match).
    pos_glyphs: Vec<Vec<GlyphId16>>,
    /// Current glyph ID for each digit slot `s` and digit `d in 0..10` at the current stage of
    /// the ascending GSUB lookup walk.
    cur_digit: Vec<[GlyphId16; 10]>,
    /// Final observed glyph IDs from Parley's 10 uniform fills, for soundness verification.
    observed_digits: Vec<[GlyphId16; 10]>,
    /// Final observed non-digit glyph IDs from Parley's uniform fills.
    observed_fixed: Vec<(usize, GlyphId16)>,
}

impl RunState {
    fn new(
        font: &FontRef<'_>,
        run_mask: &[u8],
        observed_digits: &[[u32; 10]],
        observed_fixed: &[(usize, u32)],
    ) -> Option<Self> {
        let charmap = font.charmap();
        let mut digit_slot = Vec::with_capacity(run_mask.len());
        let mut pos_glyphs = Vec::with_capacity(run_mask.len());
        let mut cur_digit = Vec::with_capacity(observed_digits.len());
        let mut obs_digits = Vec::with_capacity(observed_digits.len());
        let mut ds_idx = 0usize;

        for &b in run_mask {
            if b.is_ascii_digit() {
                let mut base = [GlyphId16::NOTDEF; 10];
                let mut obs = [GlyphId16::NOTDEF; 10];
                let mut union = Vec::with_capacity(10);
                let raw_obs = observed_digits.get(ds_idx)?;
                for d in 0..10usize {
                    let ch = (b'0' + d as u8) as char;
                    let g = to_gid16(charmap.map(ch)?)?;
                    base[d] = g;
                    obs[d] = u32_to_gid16(raw_obs[d])?;
                    push_unique(&mut union, g);
                }
                digit_slot.push(Some(ds_idx));
                cur_digit.push(base);
                obs_digits.push(obs);
                pos_glyphs.push(union);
                ds_idx += 1;
            } else {
                let base = to_gid16(charmap.map(b as char)?)?;
                digit_slot.push(None);
                pos_glyphs.push(vec![base]);
            }
        }
        if ds_idx != observed_digits.len() {
            return None;
        }
        let mut obs_fixed = Vec::with_capacity(observed_fixed.len());
        for &(pos, gid) in observed_fixed {
            if pos >= run_mask.len() {
                return None;
            }
            obs_fixed.push((pos, u32_to_gid16(gid)?));
        }
        Some(RunState {
            digit_slot,
            pos_glyphs,
            cur_digit,
            observed_digits: obs_digits,
            observed_fixed: obs_fixed,
        })
    }

    fn len(&self) -> usize {
        self.pos_glyphs.len()
    }

    fn has_digit_in(&self, range: std::ops::Range<usize>) -> bool {
        self.digit_slot[range].iter().any(Option::is_some)
    }

    fn add_glyph_at(&mut self, pos: usize, g: GlyphId16) {
        push_unique(&mut self.pos_glyphs[pos], g);
    }

    /// True if `matcher` matches at least one glyph currently reachable at `pos`.
    fn pos_matches(&self, pos: usize, matcher: &impl GlyphMatcher) -> bool {
        self.pos_glyphs[pos].iter().any(|&g| matcher.matches(g))
    }

    /// True if at digit position `pos`, `matcher` treats all 10 digits symmetrically:
    /// all 10 current digit glyphs match `matcher`.
    fn digit_symmetric_match(&self, pos: usize, matcher: &impl GlyphMatcher) -> bool {
        let Some(ds) = self.digit_slot[pos] else { return true };
        self.cur_digit[ds].iter().all(|&g| matcher.matches(g))
    }

    /// Soundness check after GSUB: every glyph ID Parley actually produced must be accounted for
    /// by our active GSUB walk.
    fn observed_accounted_for(&self) -> bool {
        for (pos, &slot) in self.digit_slot.iter().enumerate() {
            if let Some(ds) = slot {
                for d in 0..10 {
                    let obs = self.observed_digits[ds][d];
                    if !self.pos_glyphs[pos].contains(&obs) {
                        return false;
                    }
                }
            }
        }
        self.observed_fixed.iter().all(|&(pos, gid)| self.pos_glyphs[pos].contains(&gid))
    }
}

trait GlyphMatcher {
    fn matches(&self, g: GlyphId16) -> bool;
}

impl GlyphMatcher for GlyphId16 {
    fn matches(&self, g: GlyphId16) -> bool {
        *self == g
    }
}

impl GlyphMatcher for CoverageTable<'_> {
    fn matches(&self, g: GlyphId16) -> bool {
        self.get(g).is_some()
    }
}

struct ClassMatcher<'a> {
    class_def: &'a ClassDef<'a>,
    class: u16,
}

impl GlyphMatcher for ClassMatcher<'_> {
    fn matches(&self, g: GlyphId16) -> bool {
        self.class_def.get(g) == self.class
    }
}

/// First input glyph of a Format-2 Context/ChainingContext rule must satisfy both the outer
/// coverage table AND the rule set's class.
struct CoveredClassMatcher<'a> {
    coverage: &'a CoverageTable<'a>,
    class_def: &'a ClassDef<'a>,
    class: u16,
}

impl GlyphMatcher for CoveredClassMatcher<'_> {
    fn matches(&self, g: GlyphId16) -> bool {
        self.coverage.get(g).is_some() && self.class_def.get(g) == self.class
    }
}

fn u32_to_gid16(g: u32) -> Option<GlyphId16> {
    u16::try_from(g).ok().map(GlyphId16::new)
}

fn to_gid16(g: GlyphId) -> Option<GlyphId16> {
    u32_to_gid16(g.to_u32())
}

fn push_unique(v: &mut Vec<GlyphId16>, g: GlyphId16) {
    if !v.contains(&g) {
        v.push(g);
    }
}

/// Verifies that the OpenType/AAT tables of the font face used by one run cannot produce any
/// digit-dependent substitution or positioning beyond what the 10 uniform fills and order-2
/// De Bruijn probe already verified.
pub(super) fn verify_run_tables(
    font_data: &[u8],
    font_index: u32,
    run_mask: &[u8],
    style: TextStyle,
    observed_digits: &[[u32; 10]],
    observed_fixed: &[(usize, u32)],
) -> bool {
    let Ok(font) = FontRef::from_index(font_data, font_index) else {
        return false;
    };
    // Reject AAT state-machine tables: their arbitrary state transitions are not bounded by
    // OpenType lookup structure.
    for aat in [b"morx", b"mort", b"kerx"] {
        if font.table_data(Tag::new(aat)).is_some() {
            return false;
        }
    }
    // Legacy TrueType `kern` is safe only when every subtable is strictly pairwise (no AAT
    // state-machine or cross-stream subtables).
    if font.table_data(Tag::new(b"kern")).is_some() {
        let Ok(kern) = font.kern() else { return false };
        for sub in kern.subtables() {
            let Ok(sub) = sub else { return false };
            if sub.is_state_machine() || sub.is_cross_stream() {
                return false;
            }
        }
    }

    let Some(mut state) = RunState::new(&font, run_mask, observed_digits, observed_fixed) else {
        return false;
    };

    let mark_class_def = match font.gdef() {
        Ok(gdef) => match gdef.glyph_class_def() {
            Some(Ok(cd)) => Some(cd),
            Some(Err(_)) => return false,
            None => None,
        },
        Err(_) => {
            if font.table_data(Tag::new(b"GDEF")).is_some() {
                return false;
            }
            None
        }
    };
    let no_marks = |st: &RunState| -> bool {
        let Some(cd) = &mark_class_def else { return true };
        st.pos_glyphs.iter().all(|gs| gs.iter().all(|&g| cd.get(g) != 3))
    };
    if !no_marks(&state) {
        return false;
    }

    let active_tags = active_feature_tags(style);

    if font.table_data(Tag::new(b"GSUB")).is_some() {
        let Ok(gsub) = font.gsub() else { return false };
        if !verify_gsub(&gsub, &active_tags, &mut state) {
            return false;
        }
        if !no_marks(&state) {
            return false;
        }
    }
    if !state.observed_accounted_for() {
        return false;
    }

    // For GPOS, use the exact final shaped digit glyphs as `cur_digit`.
    state.cur_digit = state.observed_digits.clone();

    if font.table_data(Tag::new(b"GPOS")).is_some() {
        let Ok(gpos) = font.gpos() else { return false };
        if !verify_gpos(&gpos, &active_tags, &state) {
            return false;
        }
    }

    true
}

/// Active OpenType feature tags for plain Latin/Common LTR shaping in `style`, matching
/// HarfRust's defaults plus `shape.rs`'s overrides.
fn active_feature_tags(style: TextStyle) -> Vec<Tag> {
    let mut tags: Vec<Tag> = [
        b"abvm", b"blwm", b"calt", b"ccmp", b"clig", b"curs", b"dist", b"kern", b"liga", b"locl",
        b"ltra", b"ltrm", b"mark", b"mkmk", b"rand", b"rclt", b"rlig", b"rvrn",
    ]
    .into_iter()
    .map(Tag::new)
    .collect();

    for feat in parley::FontFeature::parse_css_list(style.features).flatten() {
        let tag = Tag::new(&feat.tag.to_bytes());
        if feat.value != 0 {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        } else {
            tags.retain(|&t| t != tag);
        }
    }
    if matches!(style.family, Family::Mono | Family::SystemMono) {
        for off in [b"liga", b"clig", b"calt", b"dlig"].map(Tag::new) {
            tags.retain(|&t| t != off);
        }
    }
    tags
}

/// Collects sorted, deduplicated lookup indices referenced by any feature record matching
/// `active_tags`, including alternate feature tables in `FeatureVariations`.
fn collect_active_lookups(
    feature_list: &FeatureList<'_>,
    feature_variations: Option<Result<FeatureVariations<'_>, skrifa::raw::ReadError>>,
    active_tags: &[Tag],
) -> Option<Vec<u16>> {
    let data = feature_list.offset_data();
    let records = feature_list.feature_records();
    let mut active_feature_indices = Vec::new();
    let mut lookups = Vec::new();

    for (fi, rec) in records.iter().enumerate() {
        if !active_tags.contains(&rec.feature_tag()) {
            continue;
        }
        active_feature_indices.push(fi as u16);
        let feat = rec.feature(data).ok()?;
        for idx in feat.lookup_list_indices() {
            lookups.push(idx.get());
        }
    }

    if let Some(fv) = feature_variations {
        let fv = fv.ok()?;
        let fv_data = fv.offset_data();
        for rec in fv.feature_variation_records() {
            let Some(subst) = rec.feature_table_substitution(fv_data) else { continue };
            let subst = subst.ok()?;
            let s_data = subst.offset_data();
            for srec in subst.substitutions() {
                if !active_feature_indices.contains(&srec.feature_index()) {
                    continue;
                }
                let alt_off = srec.alternate_feature_offset().to_u32() as usize;
                let alt_bytes = s_data.slice(alt_off..)?;
                let feat = skrifa::raw::tables::layout::Feature::read(alt_bytes, Tag::new(b"    ")).ok()?;
                for idx in feat.lookup_list_indices() {
                    lookups.push(idx.get());
                }
            }
        }
    }

    lookups.sort_unstable();
    lookups.dedup();
    Some(lookups)
}

fn flag_skips_bases_or_ligs(flag: LookupFlag) -> bool {
    flag.contains(LookupFlag::IGNORE_BASE_GLYPHS) || flag.contains(LookupFlag::IGNORE_LIGATURES)
}

fn apply_single_subst_to_gid(sub: &SingleSubst<'_>, g: GlyphId16) -> Option<Option<GlyphId16>> {
    match sub {
        SingleSubst::Format1(f1) => {
            let cov = f1.coverage().ok()?;
            if cov.get(g).is_some() {
                let next = (g.to_u16() as i32 + f1.delta_glyph_id() as i32) & 0xffff;
                Some(Some(GlyphId16::new(next as u16)))
            } else {
                Some(None)
            }
        }
        SingleSubst::Format2(f2) => {
            let cov = f2.coverage().ok()?;
            if let Some(idx) = cov.get(g) {
                let next = f2.substitute_glyph_ids().get(idx as usize)?.get();
                Some(Some(next))
            } else {
                Some(None)
            }
        }
    }
}

fn apply_single_lookup_at(
    lookup: &SubstitutionLookup<'_>,
    state: &mut RunState,
    pos: usize,
) -> bool {
    let Ok(SubstitutionSubtables::Single(subs)) = lookup.subtables() else {
        return false;
    };
    let existing = state.pos_glyphs[pos].clone();
    for sub in subs.iter() {
        let Ok(sub) = sub else { return false };
        for g in &existing {
            let Some(mapped) = apply_single_subst_to_gid(&sub, *g) else { return false };
            if let Some(next) = mapped {
                state.add_glyph_at(pos, next);
            }
        }
    }
    true
}

fn verify_gsub(gsub: &gsub::Gsub<'_>, active_tags: &[Tag], state: &mut RunState) -> bool {
    let Ok(fl) = gsub.feature_list() else { return false };
    let Ok(ll) = gsub.lookup_list() else { return false };
    let Some(active) = collect_active_lookups(&fl, gsub.feature_variations(), active_tags) else {
        return false;
    };
    let lookups = ll.lookups();

    for idx in active {
        let Ok(lookup) = lookups.get(idx as usize) else { return false };
        let flag = lookup.lookup_flag();
        let Ok(subtables) = lookup.subtables() else { return false };
        match subtables {
            SubstitutionSubtables::EmptyExtension => {}
            SubstitutionSubtables::Single(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    for pos in 0..state.len() {
                        if let Some(ds) = state.digit_slot[pos] {
                            let mut next_cur = state.cur_digit[ds];
                            let mut any_mapped = false;
                            let mut all_mapped = true;
                            for (d, slot) in next_cur.iter_mut().enumerate() {
                                let g = state.cur_digit[ds][d];
                                let Some(mapped) = apply_single_subst_to_gid(&sub, g) else { return false };
                                if let Some(next) = mapped {
                                    any_mapped = true;
                                    state.add_glyph_at(pos, next);
                                    *slot = next;
                                } else {
                                    all_mapped = false;
                                }
                            }
                            // Update `cur_digit` when this SingleSubst either applies uniformly
                            // to all 10 digits or produces an observed final digit glyph (e.g.
                            // `zero` feature replacing only `'0'`).
                            if any_mapped {
                                for (d, &next) in next_cur.iter().enumerate() {
                                    if all_mapped || next == state.observed_digits[ds][d] {
                                        state.cur_digit[ds][d] = next;
                                    }
                                }
                            }
                        } else {
                            let cur = state.pos_glyphs[pos].clone();
                            for g in cur {
                                let Some(mapped) = apply_single_subst_to_gid(&sub, g) else { return false };
                                if let Some(next) = mapped {
                                    state.add_glyph_at(pos, next);
                                }
                            }
                        }
                    }
                }
            }
            SubstitutionSubtables::Multiple(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.coverage() else { return false };
                    if (0..state.len()).any(|p| state.pos_matches(p, &cov)) {
                        return false;
                    }
                }
            }
            SubstitutionSubtables::Alternate(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.coverage() else { return false };
                    let sets = sub.alternate_sets();
                    for pos in 0..state.len() {
                        if let Some(ds) = state.digit_slot[pos] {
                            for d in 0..10 {
                                let g = state.cur_digit[ds][d];
                                if let Some(ci) = cov.get(g) {
                                    let Ok(aset) = sets.get(ci as usize) else { return false };
                                    for alt in aset.alternate_glyph_ids() {
                                        let a = alt.get();
                                        state.add_glyph_at(pos, a);
                                        if a == state.observed_digits[ds][d] {
                                            state.cur_digit[ds][d] = a;
                                        }
                                    }
                                }
                            }
                        } else {
                            let cur = state.pos_glyphs[pos].clone();
                            for g in cur {
                                if let Some(ci) = cov.get(g) {
                                    let Ok(aset) = sets.get(ci as usize) else { return false };
                                    for alt in aset.alternate_glyph_ids() {
                                        state.add_glyph_at(pos, alt.get());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            SubstitutionSubtables::Ligature(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.coverage() else { return false };
                    let lsets = sub.ligature_sets();
                    for pos in 0..state.len() {
                        for &g in &state.pos_glyphs[pos] {
                            let Some(ci) = cov.get(g) else { continue };
                            if flag_skips_bases_or_ligs(flag) {
                                return false;
                            }
                            let Ok(lset) = lsets.get(ci as usize) else { return false };
                            for lig in lset.ligatures().iter() {
                                let Ok(lig) = lig else { return false };
                                let comps = lig.component_glyph_ids();
                                let end = pos + 1 + comps.len();
                                if end > state.len() || !state.has_digit_in(pos..end) {
                                    continue;
                                }
                                let matched = comps.iter().enumerate().all(|(k, c)| {
                                    state.pos_glyphs[pos + 1 + k].contains(&c.get())
                                });
                                if matched {
                                    return false;
                                }
                            }
                        }
                    }
                }
            }
            SubstitutionSubtables::Contextual(subs) => {
                let mut nested_single: Vec<(usize, u16)> = Vec::new();
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    if !check_sequence_context(&sub, flag, state, &mut nested_single) {
                        return false;
                    }
                }
                for (pos, nested_idx) in nested_single {
                    let Ok(nested) = lookups.get(nested_idx as usize) else { return false };
                    if !apply_single_lookup_at(&nested, state, pos) {
                        return false;
                    }
                }
            }
            SubstitutionSubtables::ChainContextual(subs) => {
                let mut nested_single: Vec<(usize, u16)> = Vec::new();
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    if !check_chained_context(&sub, flag, state, &mut nested_single) {
                        return false;
                    }
                }
                for (pos, nested_idx) in nested_single {
                    let Ok(nested) = lookups.get(nested_idx as usize) else { return false };
                    if !apply_single_lookup_at(&nested, state, pos) {
                        return false;
                    }
                }
            }
            SubstitutionSubtables::Reverse(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.coverage() else { return false };
                    let b_covs: Vec<CoverageTable<'_>> = match sub.backtrack_coverages().iter().collect::<Result<_, _>>() {
                        Ok(v) => v,
                        Err(_) => return false,
                    };
                    let l_covs: Vec<CoverageTable<'_>> = match sub.lookahead_coverages().iter().collect::<Result<_, _>>() {
                        Ok(v) => v,
                        Err(_) => return false,
                    };
                    let subst_gids = sub.substitute_glyph_ids();
                    for pos in 0..state.len() {
                        if !state.pos_matches(pos, &cov) {
                            continue;
                        }
                        if flag_skips_bases_or_ligs(flag) {
                            return false;
                        }
                        if pos < b_covs.len() || pos + 1 + l_covs.len() > state.len() {
                            continue;
                        }
                        let b_ok = b_covs.iter().enumerate().all(|(k, c)| state.pos_matches(pos - 1 - k, c));
                        let l_ok = l_covs.iter().enumerate().all(|(k, c)| state.pos_matches(pos + 1 + k, c));
                        if !(b_ok && l_ok) {
                            continue;
                        }
                        let win = (pos - b_covs.len())..(pos + 1 + l_covs.len());
                        if state.has_digit_in(win) {
                            if state.digit_slot[pos].is_some() {
                                return false;
                            }
                            let b_sym = b_covs.iter().enumerate().all(|(k, c)| state.digit_symmetric_match(pos - 1 - k, c));
                            let l_sym = l_covs.iter().enumerate().all(|(k, c)| state.digit_symmetric_match(pos + 1 + k, c));
                            if !(b_sym && l_sym) {
                                return false;
                            }
                        }
                        let cur = state.pos_glyphs[pos].clone();
                        for g in cur {
                            if let Some(ci) = cov.get(g) {
                                let Some(rep) = subst_gids.get(ci as usize) else { return false };
                                state.add_glyph_at(pos, rep.get());
                            }
                        }
                    }
                }
            }
        }
    }
    true
}

fn verify_gpos(gpos: &gpos::Gpos<'_>, active_tags: &[Tag], state: &RunState) -> bool {
    let Ok(fl) = gpos.feature_list() else { return false };
    let Ok(ll) = gpos.lookup_list() else { return false };
    let Some(active) = collect_active_lookups(&fl, gpos.feature_variations(), active_tags) else {
        return false;
    };
    let lookups = ll.lookups();

    for idx in active {
        let Ok(lookup) = lookups.get(idx as usize) else { return false };
        let flag = lookup.lookup_flag();
        let Ok(subtables) = lookup.subtables() else { return false };
        match subtables {
            PositionSubtables::EmptyExtension | PositionSubtables::Single(_) => {}
            PositionSubtables::Pair(_) => {
                // PairPos is exhaustively tested across all adjacent pairs by the 10 uniform
                // fills and the 102-byte De Bruijn probe, provided it does not skip base glyphs
                // or ligatures to kern non-adjacent slots.
                if flag_skips_bases_or_ligs(flag) {
                    return false;
                }
            }
            PositionSubtables::Cursive(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.coverage() else { return false };
                    if (0..state.len()).any(|p| state.pos_matches(p, &cov)) {
                        return false;
                    }
                }
            }
            PositionSubtables::MarkToBase(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.mark_coverage() else { return false };
                    if (0..state.len()).any(|p| state.pos_matches(p, &cov)) {
                        return false;
                    }
                }
            }
            PositionSubtables::MarkToLig(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov) = sub.mark_coverage() else { return false };
                    if (0..state.len()).any(|p| state.pos_matches(p, &cov)) {
                        return false;
                    }
                }
            }
            PositionSubtables::MarkToMark(subs) => {
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    let Ok(cov1) = sub.mark1_coverage() else { return false };
                    if (0..state.len()).any(|p| state.pos_matches(p, &cov1)) {
                        return false;
                    }
                }
            }
            PositionSubtables::Contextual(subs) => {
                let mut nested: Vec<(usize, u16)> = Vec::new();
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    if !check_sequence_context(&sub, flag, state, &mut nested) {
                        return false;
                    }
                }
                if !check_nested_gpos_single(&lookups, &nested) {
                    return false;
                }
            }
            PositionSubtables::ChainContextual(subs) => {
                let mut nested: Vec<(usize, u16)> = Vec::new();
                for sub in subs.iter() {
                    let Ok(sub) = sub else { return false };
                    if !check_chained_context(&sub, flag, state, &mut nested) {
                        return false;
                    }
                }
                if !check_nested_gpos_single(&lookups, &nested) {
                    return false;
                }
            }
        }
    }
    true
}

fn check_nested_gpos_single<'a>(
    lookups: &skrifa::raw::ArrayOfOffsets<'a, PositionLookup<'a>, skrifa::raw::types::Offset16>,
    nested: &[(usize, u16)],
) -> bool {
    for &(_, idx) in nested {
        let Ok(l) = lookups.get(idx as usize) else { return false };
        if !matches!(l.subtables(), Ok(PositionSubtables::Single(_) | PositionSubtables::EmptyExtension)) {
            return false;
        }
    }
    true
}

/// Checks one matched contextual window: if the window overlaps any digit slot, every digit slot
/// in the window must match all 10 digits symmetrically and no nested lookup may target a digit
/// slot. Records every nested lookup targeting a non-digit position into `nested_out`.
#[allow(clippy::too_many_arguments)]
fn check_matched_window<B: GlyphMatcher, I: GlyphMatcher, L: GlyphMatcher>(
    state: &RunState,
    pos: usize,
    backtrack: &[B],
    first_input: &impl GlyphMatcher,
    rest_input: &[I],
    lookahead: &[L],
    records: &[SequenceLookupRecord],
    nested_out: &mut Vec<(usize, u16)>,
) -> bool {
    let input_len = 1 + rest_input.len();
    let win_start = pos - backtrack.len();
    let win_end = pos + input_len + lookahead.len();
    let overlaps_digit = state.has_digit_in(win_start..win_end);

    if overlaps_digit {
        let b_sym = backtrack.iter().enumerate().all(|(k, m)| state.digit_symmetric_match(pos - 1 - k, m));
        let i0_sym = state.digit_symmetric_match(pos, first_input);
        let ir_sym = rest_input.iter().enumerate().all(|(k, m)| state.digit_symmetric_match(pos + 1 + k, m));
        let l_sym = lookahead.iter().enumerate().all(|(k, m)| state.digit_symmetric_match(pos + input_len + k, m));
        if !(b_sym && i0_sym && ir_sym && l_sym) {
            return false;
        }
    }
    for rec in records {
        let target = pos + rec.sequence_index() as usize;
        if target >= state.len() || state.digit_slot[target].is_some() {
            return false;
        }
        nested_out.push((target, rec.lookup_list_index()));
    }
    true
}

fn check_sequence_context(
    ctx: &SequenceContext<'_>,
    flag: LookupFlag,
    state: &RunState,
    nested_out: &mut Vec<(usize, u16)>,
) -> bool {
    const EMPTY: &[GlyphId16] = &[];
    match ctx {
        SequenceContext::Format1(f1) => {
            let Ok(cov) = f1.coverage() else { return false };
            let sets = f1.seq_rule_sets();
            for pos in 0..state.len() {
                for &g in &state.pos_glyphs[pos] {
                    let Some(ci) = cov.get(g) else { continue };
                    if flag_skips_bases_or_ligs(flag) {
                        return false;
                    }
                    let Some(rset) = sets.get(ci as usize) else { continue };
                    let Ok(rset) = rset else { return false };
                    for rule in rset.seq_rules().iter() {
                        let Ok(rule) = rule else { return false };
                        let rest: Vec<GlyphId16> = rule.input_sequence().iter().map(|v| v.get()).collect();
                        if pos + 1 + rest.len() > state.len() {
                            continue;
                        }
                        if rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                            && !check_matched_window(state, pos, EMPTY, &g, &rest, EMPTY, rule.seq_lookup_records(), nested_out)
                        {
                            return false;
                        }
                    }
                }
            }
            true
        }
        SequenceContext::Format2(f2) => {
            let Ok(cov) = f2.coverage() else { return false };
            let Ok(cd) = f2.class_def() else { return false };
            let sets = f2.class_seq_rule_sets();
            for pos in 0..state.len() {
                let mut seen_classes = Vec::new();
                for &g in &state.pos_glyphs[pos] {
                    if cov.get(g).is_none() {
                        continue;
                    }
                    if flag_skips_bases_or_ligs(flag) {
                        return false;
                    }
                    let cls = cd.get(g);
                    if seen_classes.contains(&cls) {
                        continue;
                    }
                    seen_classes.push(cls);
                    let Some(rset) = sets.get(cls as usize) else { continue };
                    let Ok(rset) = rset else { return false };
                    let first_m = CoveredClassMatcher { coverage: &cov, class_def: &cd, class: cls };
                    for rule in rset.class_seq_rules().iter() {
                        let Ok(rule) = rule else { return false };
                        let rest: Vec<ClassMatcher<'_>> = rule
                            .input_sequence()
                            .iter()
                            .map(|c| ClassMatcher { class_def: &cd, class: c.get() })
                            .collect();
                        if pos + 1 + rest.len() > state.len() {
                            continue;
                        }
                        if rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                            && !check_matched_window(state, pos, EMPTY, &first_m, &rest, EMPTY, rule.seq_lookup_records(), nested_out)
                        {
                            return false;
                        }
                    }
                }
            }
            true
        }
        SequenceContext::Format3(f3) => {
            let covs: Vec<CoverageTable<'_>> = match f3.coverages().iter().collect::<Result<_, _>>() {
                Ok(v) => v,
                Err(_) => return false,
            };
            let Some((first, rest)) = covs.split_first() else { return true };
            for pos in 0..state.len() {
                if !state.pos_matches(pos, first) {
                    continue;
                }
                if flag_skips_bases_or_ligs(flag) {
                    return false;
                }
                if pos + covs.len() > state.len() {
                    continue;
                }
                if rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                    && !check_matched_window(state, pos, EMPTY, first, rest, EMPTY, f3.seq_lookup_records(), nested_out)
                {
                    return false;
                }
            }
            true
        }
    }
}

fn check_chained_context(
    ctx: &ChainedSequenceContext<'_>,
    flag: LookupFlag,
    state: &RunState,
    nested_out: &mut Vec<(usize, u16)>,
) -> bool {
    match ctx {
        ChainedSequenceContext::Format1(f1) => {
            let Ok(cov) = f1.coverage() else { return false };
            let sets = f1.chained_seq_rule_sets();
            for pos in 0..state.len() {
                for &g in &state.pos_glyphs[pos] {
                    let Some(ci) = cov.get(g) else { continue };
                    if flag_skips_bases_or_ligs(flag) {
                        return false;
                    }
                    let Some(rset) = sets.get(ci as usize) else { continue };
                    let Ok(rset) = rset else { return false };
                    for rule in rset.chained_seq_rules().iter() {
                        let Ok(rule) = rule else { return false };
                        let back: Vec<GlyphId16> = rule.backtrack_sequence().iter().map(|v| v.get()).collect();
                        let rest: Vec<GlyphId16> = rule.input_sequence().iter().map(|v| v.get()).collect();
                        let look: Vec<GlyphId16> = rule.lookahead_sequence().iter().map(|v| v.get()).collect();
                        let input_len = 1 + rest.len();
                        if pos < back.len() || pos + input_len + look.len() > state.len() {
                            continue;
                        }
                        let ok = back.iter().enumerate().all(|(k, m)| state.pos_matches(pos - 1 - k, m))
                            && rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                            && look.iter().enumerate().all(|(k, m)| state.pos_matches(pos + input_len + k, m));
                        if ok && !check_matched_window(state, pos, &back, &g, &rest, &look, rule.seq_lookup_records(), nested_out) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        ChainedSequenceContext::Format2(f2) => {
            let Ok(cov) = f2.coverage() else { return false };
            let Ok(b_cd) = f2.backtrack_class_def() else { return false };
            let Ok(i_cd) = f2.input_class_def() else { return false };
            let Ok(l_cd) = f2.lookahead_class_def() else { return false };
            let sets = f2.chained_class_seq_rule_sets();
            for pos in 0..state.len() {
                let mut seen_classes = Vec::new();
                for &g in &state.pos_glyphs[pos] {
                    if cov.get(g).is_none() {
                        continue;
                    }
                    if flag_skips_bases_or_ligs(flag) {
                        return false;
                    }
                    let cls = i_cd.get(g);
                    if seen_classes.contains(&cls) {
                        continue;
                    }
                    seen_classes.push(cls);
                    let Some(rset) = sets.get(cls as usize) else { continue };
                    let Ok(rset) = rset else { return false };
                    let first_m = CoveredClassMatcher { coverage: &cov, class_def: &i_cd, class: cls };
                    for rule in rset.chained_class_seq_rules().iter() {
                        let Ok(rule) = rule else { return false };
                        let back: Vec<ClassMatcher<'_>> = rule
                            .backtrack_sequence()
                            .iter()
                            .map(|c| ClassMatcher { class_def: &b_cd, class: c.get() })
                            .collect();
                        let rest: Vec<ClassMatcher<'_>> = rule
                            .input_sequence()
                            .iter()
                            .map(|c| ClassMatcher { class_def: &i_cd, class: c.get() })
                            .collect();
                        let look: Vec<ClassMatcher<'_>> = rule
                            .lookahead_sequence()
                            .iter()
                            .map(|c| ClassMatcher { class_def: &l_cd, class: c.get() })
                            .collect();
                        let input_len = 1 + rest.len();
                        if pos < back.len() || pos + input_len + look.len() > state.len() {
                            continue;
                        }
                        let ok = back.iter().enumerate().all(|(k, m)| state.pos_matches(pos - 1 - k, m))
                            && rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                            && look.iter().enumerate().all(|(k, m)| state.pos_matches(pos + input_len + k, m));
                        if ok && !check_matched_window(state, pos, &back, &first_m, &rest, &look, rule.seq_lookup_records(), nested_out) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        ChainedSequenceContext::Format3(f3) => {
            let back: Vec<CoverageTable<'_>> = match f3.backtrack_coverages().iter().collect::<Result<_, _>>() {
                Ok(v) => v,
                Err(_) => return false,
            };
            let input: Vec<CoverageTable<'_>> = match f3.input_coverages().iter().collect::<Result<_, _>>() {
                Ok(v) => v,
                Err(_) => return false,
            };
            let look: Vec<CoverageTable<'_>> = match f3.lookahead_coverages().iter().collect::<Result<_, _>>() {
                Ok(v) => v,
                Err(_) => return false,
            };
            let Some((first, rest)) = input.split_first() else { return true };
            let input_len = input.len();
            for pos in 0..state.len() {
                if !state.pos_matches(pos, first) {
                    continue;
                }
                if flag_skips_bases_or_ligs(flag) {
                    return false;
                }
                if pos < back.len() || pos + input_len + look.len() > state.len() {
                    continue;
                }
                let ok = back.iter().enumerate().all(|(k, m)| state.pos_matches(pos - 1 - k, m))
                    && rest.iter().enumerate().all(|(k, m)| state.pos_matches(pos + 1 + k, m))
                    && look.iter().enumerate().all(|(k, m)| state.pos_matches(pos + input_len + k, m));
                if ok && !check_matched_window(state, pos, &back, first, rest, &look, f3.seq_lookup_records(), nested_out) {
                    return false;
                }
            }
            true
        }
    }
}
