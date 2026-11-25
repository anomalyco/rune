fn reading(i: usize, k: u64) -> String {
    let v = 100.0 + 80.0 * ((i as f64) * 0.37 + (k as f64) * 0.113).sin();
    format!("{v:06.2} bar")
}

fn glyph_rects_bits(instances: &[crate::render::Instance]) -> Vec<[u32; 4]> {
    instances
        .iter()
        .filter(|inst| inst.params[1] == crate::render::KIND_GLYPH)
        .map(|inst| inst.rect.map(f32::to_bits))
        .collect()
}

fn glyph_uvs_bits(instances: &[crate::render::Instance]) -> Vec<[u32; 4]> {
    instances
        .iter()
        .filter(|inst| inst.params[1] == crate::render::KIND_GLYPH)
        .map(|inst| inst.uv.map(f32::to_bits))
        .collect()
}

#[test]
fn tabular_live_text_500_cell_grid_drops_per_tick_shaping_to_zero_with_bit_identical_quads() {
    const N: usize = 500;
    const COLS: usize = 20;
    struct Panel {
        cells: Vec<crate::LiveText>,
        tick: u64,
        rebuild: bool,
    }
    fn view(s: &Panel, _: &mut crate::Cx<Panel>) -> crate::El {
        let style = crate::TextStyle::new(12.0, 400.0).features("'tnum' 1");
        let mut grid = crate::col().gap(2.0);
        for r in 0..s.cells.len().div_ceil(COLS) {
            let mut line = crate::row().gap(2.0);
            for i in r * COLS..((r + 1) * COLS).min(s.cells.len()) {
                let v = if s.rebuild {
                    crate::text(reading(i, s.tick))
                } else {
                    crate::text("000.00 bar").live_text(&s.cells[i])
                };
                line = line.child(
                    crate::col()
                        .w(70.0)
                        .h(18.0)
                        .child(v.key(format!("c{i}")).font(style)),
                );
            }
            grid = grid.child(line);
        }
        crate::col().w_full().h_full().padding(10.0).child(grid)
    }

    let cells: Vec<crate::LiveText> = (0..N).map(|i| crate::LiveText::new(reading(i, 0))).collect();
    let handles = cells.clone();
    let mut live_rt = crate::Runtime::new(
        Panel { cells, tick: 0, rebuild: false },
        view,
        (1500.0, 600.0),
        crate::Env::new(true, 2.0),
    );
    live_rt.frame(0.0);
    let layouts_after_mount = live_rt.core().text.cached_layout_count();

    // Tick 1 verifies the mask once; ticks 2..=5 must shape 0 texts for all 500 changed cells.
    let mut prev_glyph_rects = glyph_rects_bits(&live_rt.instances);
    let mut prev_glyph_uvs = glyph_uvs_bits(&live_rt.instances);
    for k in 1..=5u64 {
        for (i, c) in handles.iter().enumerate() {
            c.set(reading(i, k));
        }
        live_rt.frame(k as f64 / 60.0);
        if k >= 2 {
            assert_eq!(
                live_rt.stats.shaped,
                0,
                "tick {k}: verified tabular LiveText grid must shape 0 times for {N} updated cells"
            );
        }
        let cur_rects = glyph_rects_bits(&live_rt.instances);
        let cur_uvs = glyph_uvs_bits(&live_rt.instances);
        assert!(
            cur_rects != prev_glyph_rects || cur_uvs != prev_glyph_uvs,
            "tick {k}: painted quads must update when digits change (no stale quad-cache hit)"
        );
        prev_glyph_rects = cur_rects;
        prev_glyph_uvs = cur_uvs;
    }
    assert_eq!(
        live_rt.core().text.cached_layout_count(),
        layouts_after_mount,
        "tabular LiveText updates must not grow shared layout cache"
    );

    // Between ticks (no value change), instances stay bit-identical (copied by paint keep).
    let tick5_rects = glyph_rects_bits(&live_rt.instances);
    let tick5_uvs = glyph_uvs_bits(&live_rt.instances);
    live_rt.frame(6.0 / 60.0);
    assert_eq!(glyph_rects_bits(&live_rt.instances), tick5_rects);
    assert_eq!(glyph_uvs_bits(&live_rt.instances), tick5_uvs);

    // Compare painted glyph placement bit-for-bit against a full-reshaped Runtime at tick 5.
    let mut ref_rt = crate::Runtime::new(
        Panel { cells: (0..N).map(|_| crate::LiveText::new("")).collect(), tick: 5, rebuild: true },
        view,
        (1500.0, 600.0),
        crate::Env::new(true, 2.0),
    );
    ref_rt.frame(0.0);
    assert_eq!(
        glyph_rects_bits(&live_rt.instances),
        glyph_rects_bits(&ref_rt.instances),
        "500-cell tabular fast path must emit bit-identical glyph rects to full shaping"
    );

    // Accessibility tree and web mirror both expose the latest tick-5 reading.
    let expected_c0 = reading(0, 5);
    let expected_c499 = reading(499, 5);
    let tree = live_rt.accessibility_tree();
    let ak_text = |key: &str| {
        let node = &tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).unwrap().1;
        node.value().or_else(|| node.label()).map(str::to_owned)
    };
    assert_eq!(ak_text("c0").as_deref(), Some(expected_c0.as_str()));
    assert_eq!(ak_text("c499").as_deref(), Some(expected_c499.as_str()));
    let json = live_rt.a11y_json();
    assert!(json.contains(&expected_c0));
    assert!(json.contains(&expected_c499));
}

#[test]
fn tabular_live_text_handles_fallback_transitions_selectable_a11y_and_font_switch() {
    struct State {
        val: crate::LiveText,
        sel: crate::LiveText,
        prop: crate::LiveText,
        plain_val: String,
        plain_sel: String,
        plain_prop: String,
        rebuild: bool,
    }
    fn view(s: &State, _: &mut crate::Cx<State>) -> crate::El {
        let tnum = crate::TextStyle::new(13.0, 400.0).features("'tnum' 1");
        let prop = crate::TextStyle::new(13.0, 400.0);
        let a = if s.rebuild {
            crate::text(s.plain_val.clone())
        } else {
            crate::text("000.00 bar").live_text(&s.val)
        };
        let b = if s.rebuild {
            crate::text(s.plain_sel.clone())
        } else {
            crate::text("00:00").live_text(&s.sel)
        };
        let c = if s.rebuild {
            crate::text(s.plain_prop.clone())
        } else {
            crate::text("000.00 bar").live_text(&s.prop)
        };
        crate::col()
            .size(320.0, 160.0)
            .gap(6.0)
            .child(a.key("val").size(160.0, 22.0).font(tnum))
            .child(b.key("sel").size(160.0, 22.0).font(tnum).selectable(true))
            .child(c.key("prop").size(160.0, 22.0).font(prop))
    }

    let make = |rebuild: bool, v: &str, sv: &str, pv: &str| {
        crate::Runtime::new(
            State {
                val: crate::LiveText::new(v),
                sel: crate::LiveText::new(sv),
                prop: crate::LiveText::new(pv),
                plain_val: v.to_owned(),
                plain_sel: sv.to_owned(),
                plain_prop: pv.to_owned(),
                rebuild,
            },
            view,
            (320.0, 160.0),
            crate::Env::new(true, 2.0),
        )
    };

    let mut rt = make(false, "100.00 bar", "10:00", "111.11 bar");
    rt.frame(0.0);

    let steps = [
        ("123.45 bar", "12:34", "888.88 bar", true),
        ("987.65 bar", "56:78", "222.22 bar", true),
        ("FAULT", "99:99", "333.33 bar", false),
        ("الضغط 123.45 بار", "01:23", "444.44 bar", false),
        ("456.78 bar", "23:45", "555.55 bar", true),
    ];
    for (step, &(v, sv, pv, expect_val_tabular)) in steps.iter().enumerate() {
        rt.state.val.set(v);
        rt.state.sel.set(sv);
        rt.state.prop.set(pv);
        rt.frame((step + 1) as f64 / 60.0);

        let val_has_tab = rt.core().tree.nodes.iter()
            .find(|n| n.key.as_deref() == Some("val"))
            .unwrap()
            .cold
            .get()
            .is_some_and(|c| c.tabular_layout.is_some());
        let prop_has_tab = rt.core().tree.nodes.iter()
            .find(|n| n.key.as_deref() == Some("prop"))
            .unwrap()
            .cold
            .get()
            .is_some_and(|c| c.tabular_layout.is_some());
        assert_eq!(val_has_tab, expect_val_tabular, "step {step}: val={v:?}");
        assert!(!prop_has_tab, "step {step}: proportional text must never use tabular fast path");

        let mut fresh = make(true, v, sv, pv);
        fresh.frame(0.0);
        assert_eq!(
            glyph_rects_bits(&rt.instances),
            glyph_rects_bits(&fresh.instances),
            "step {step}: live_text glyph rects must match full shaping for val={v:?} sel={sv:?} prop={pv:?}"
        );

        // Selectable tabular LiveText must expose identical AccessKit TextRun geometry & value.
        let a11y_live = rt.accessibility_tree();
        let a11y_ref = fresh.accessibility_tree();
        let run_of = |tree: &accesskit::TreeUpdate| {
            let sel = &tree.nodes.iter().find(|(_, n)| n.author_id() == Some("sel")).unwrap().1;
            let child_id = sel.children()[0];
            let run = &tree.nodes.iter().find(|(id, _)| *id == child_id).unwrap().1;
            (
                run.value().map(str::to_owned),
                run.character_lengths().to_vec(),
                run.character_positions().map(|s| s.iter().map(|f| f.to_bits()).collect::<Vec<_>>()),
                run.character_widths().map(|s| s.iter().map(|f| f.to_bits()).collect::<Vec<_>>()),
            )
        };
        assert_eq!(run_of(&a11y_live), run_of(&a11y_ref), "step {step}: selectable a11y TextRun mismatch");
    }

    // Switching font families clears tabular templates and redraws cleanly in the new family.
    rt.core_mut().set_font_families("Inter", "Inter");
    rt.frame(10.0 / 60.0);
    rt.state.val.set("321.09 bar");
    rt.frame(11.0 / 60.0);
    let mut fresh = make(true, "321.09 bar", "23:45", "555.55 bar");
    fresh.frame(0.0);
    assert_eq!(glyph_rects_bits(&rt.instances), glyph_rects_bits(&fresh.instances));
}

#[test]
fn tabular_live_text_survives_memo_reuse_across_commits_without_reshaping() {
    struct Panel {
        cells: Vec<crate::LiveText>,
        dots: usize,
        tick: u64,
        rebuild: bool,
    }
    fn view(s: &Panel, _: &mut crate::Cx<Panel>) -> crate::El {
        let style = crate::TextStyle::new(12.0, 400.0).features("'tnum' 1");
        let grid = if s.rebuild {
            let mut g = crate::col().gap(2.0);
            for i in 0..s.cells.len() {
                g = g.child(crate::text(reading(i, s.tick)).key(format!("c{i}")).w(80.0).h(18.0).font(style));
            }
            g
        } else {
            crate::memo("grid", s.cells.len(), || {
                let mut g = crate::col().gap(2.0);
                for (i, c) in s.cells.iter().enumerate() {
                    g = g.child(crate::text("000.00 bar").live_text(c).key(format!("c{i}")).w(80.0).h(18.0).font(style));
                }
                g
            })
        };
        let status = (0..s.dots).fold(crate::row().gap(4.0), |r, _| {
            r.child(crate::el().size(6.0, 6.0).fill(crate::Color::WHITE))
        });
        crate::col().w_full().h_full().padding(8.0).child(status).child(grid)
    }

    let cells: Vec<crate::LiveText> = (0..16).map(|i| crate::LiveText::new(reading(i, 0))).collect();
    let handles = cells.clone();
    let mut rt = crate::Runtime::new(
        Panel { cells, dots: 1, tick: 0, rebuild: false },
        view,
        (300.0, 400.0),
        crate::Env::new(true, 2.0),
    );
    rt.frame(0.0);

    // Tick 1 verifies the tabular mask and populates tabular_layout on all 16 cells.
    for (i, c) in handles.iter().enumerate() {
        c.set(reading(i, 1));
    }
    rt.frame(1.0 / 60.0);

    // Commit outside the memo WITHOUT changing the LiveText cells: memo reuse must carry
    // tabular_layout intact so cells keep drawing tick-1 readings (not "000.00 bar") with 0 shapes.
    rt.update(|s, _| s.dots = 3);
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.stats.shaped, 0, "memo-reused unchanged tabular LiveText must shape 0 times");
    let mut ref_t1 = crate::Runtime::new(
        Panel { cells: (0..16).map(|_| crate::LiveText::new("")).collect(), dots: 3, tick: 1, rebuild: true },
        view,
        (300.0, 400.0),
        crate::Env::new(true, 2.0),
    );
    ref_t1.frame(0.0);
    assert_eq!(
        glyph_rects_bits(&rt.instances),
        glyph_rects_bits(&ref_t1.instances),
        "memo reuse must keep latched tabular digits, not revert to template placeholder"
    );

    // Commit outside the memo AND update all 16 LiveText cells on the same tick: 0 shapes!
    rt.update(|s, _| s.dots = 5);
    for (i, c) in handles.iter().enumerate() {
        c.set(reading(i, 2));
    }
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.stats.shaped, 0, "memo-reused + updated tabular LiveText tick must shape 0 times");
    let mut ref_t2 = crate::Runtime::new(
        Panel { cells: (0..16).map(|_| crate::LiveText::new("")).collect(), dots: 5, tick: 2, rebuild: true },
        view,
        (300.0, 400.0),
        crate::Env::new(true, 2.0),
    );
    ref_t2.frame(0.0);
    assert_eq!(
        glyph_rects_bits(&rt.instances),
        glyph_rects_bits(&ref_t2.instances),
        "memo-reused + updated tabular LiveText must match full shaping bit-for-bit"
    );
}
