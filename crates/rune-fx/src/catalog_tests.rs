use super::*;

#[test]
fn styles_are_in_variant_order_with_unique_ids() {
    for (k, s) in fill_styles().iter().enumerate() {
        assert_eq!(s.default_fill().style.index(), k, "{}", s.id);
        assert_eq!(s.default_fill().info().id, s.id);
        assert_eq!(fill_style(s.id).map(|x| x.id), Some(s.id));
        assert!(!s.presets.is_empty() && !s.params.is_empty());
        let mut ids: Vec<_> = s.params.iter().map(|p| p.id).chain(FillParams::COMMON.iter().map(|p| p.id)).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), s.params.len() + FillParams::COMMON.len(), "{}: unique param ids", s.id);
        let mut ps: Vec<_> = s.presets.iter().map(|p| p.id).collect();
        ps.sort();
        ps.dedup();
        assert_eq!(ps.len(), s.presets.len());
    }
    let mut ids: Vec<_> = fill_styles().iter().map(|s| s.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), fill_styles().len());
    assert!(fill_style("nope").is_none());
}

#[test]
fn first_preset_is_the_default() {
    let d = [
        FillStyle::from(LiquidMetalParams::default()),
        MeshGradientParams::default().into(),
        DitherParams::default().into(),
        GrainGradientParams::default().into(),
        FlutedGlassParams::default().into(),
        MetaballParams::default().into(),
        SwirlParams::default().into(),
        VoronoiParams::default().into(),
        WaveParams::default().into(),
        HalftoneParams::default().into(),
        PlasticParams::default().into(),
    ];
    assert_eq!(d.len(), fill_styles().len());
    for (s, d) in fill_styles().iter().zip(d) {
        assert_eq!(s.presets[0].style(), d, "{}", s.id);
    }
}

#[test]
fn every_param_gets_sets_and_sits_in_range() {
    for s in fill_styles() {
        for pr in s.presets {
            let f = pr.fill();
            for p in s.params.iter().chain(FillParams::COMMON) {
                let v = f.get(p.id).unwrap_or_else(|| panic!("{}.{} reads", s.id, p.id));
                match (p.kind, v) {
                    (ParamKind::Number { min, max, .. }, Value::Number(x)) => assert!((min..=max).contains(&x), "{}/{}.{} = {x} outside {min}..{max}", s.id, pr.id, p.id),
                    (ParamKind::Count { min, max }, Value::Count(n)) => assert!((min..=max).contains(&n), "{}.{}", s.id, p.id),
                    (ParamKind::Choice(c), Value::Choice(k)) => assert!(k < c.len()),
                    (ParamKind::Colors { min, max }, Value::Colors(c)) => assert!((min..=max).contains(&c.len()), "{}/{}.{}", s.id, pr.id, p.id),
                    (ParamKind::Toggle, Value::Toggle(_)) | (ParamKind::Color, Value::Color(_)) | (ParamKind::Point, Value::Point(_)) | (ParamKind::Points { .. }, Value::Points(_)) => {}
                    (k, v) => panic!("{}.{}: {k:?} holds {v:?}", s.id, p.id),
                }
                let mut g = f;
                assert!(g.set(p.id, v), "{}.{} writes", s.id, p.id);
                assert_eq!(g, f);
                assert!(!g.set(p.id, Value::Point([9.0, 9.0])) || matches!(p.kind, ParamKind::Point), "{}.{} rejects the wrong kind", s.id, p.id);
                assert!(!g.set(p.id, Value::Number(f32::NAN)), "{}.{} rejects NaN", s.id, p.id);
                assert!(!g.set(p.id, Value::Number(f32::INFINITY)), "{}.{} rejects inf", s.id, p.id);
                // Round-trip a distinct value for every param kind.
                let alt = match (p.kind, v) {
                    (ParamKind::Number { min, max, .. }, Value::Number(x)) => Value::Number(if (x - max).abs() > 1e-4 { max } else { min }),
                    (ParamKind::Count { min, max }, Value::Count(n)) => Value::Count(if n != max { max } else { min }),
                    (ParamKind::Choice(c), Value::Choice(k)) => Value::Choice((k + 1) % c.len()),
                    (ParamKind::Toggle, Value::Toggle(b)) => Value::Toggle(!b),
                    (ParamKind::Color, _) => Value::Color(rgb(0x123456)),
                    (ParamKind::Colors { max, .. }, _) => Value::Colors(Stops::hex(&[0x112233, 0x445566, 0x778899, 0xaabbcc, 0xddeeff, 0x102030][..max])),
                    (ParamKind::Point, _) => Value::Point([0.25, 0.75]),
                    (ParamKind::Points { .. }, _) => Value::Points([[0.1, 0.2]; MAX_STOPS]),
                    _ => unreachable!(),
                };
                assert_ne!(alt, v, "{}.{} has a distinct test value", s.id, p.id);
                assert!(g.set(p.id, alt), "{}.{} writes distinct value", s.id, p.id);
                assert_eq!(g.get(p.id), Some(alt), "{}.{} round-trips distinct value", s.id, p.id);
                assert_ne!(g, f, "{}.{} mutates FillParams", s.id, p.id);
            }
            assert!(f.get("nope").is_none());
        }
    }
}

#[test]
fn changing_a_param_changes_the_fill() {
    let mut f = Fx::fill(LiquidMetalParams::gold());
    let Fx::Fill(p) = &mut f else { unreachable!() };
    assert!(p.set("flow", Value::Number(0.9)));
    assert!(p.set("tint", Value::Color(rgb(0x808080))));
    assert!(p.set("border", Value::Number(3.0)));
    let FillStyle::LiquidMetal(m) = p.style else { unreachable!() };
    assert_eq!(m.flow, 0.9);
    assert_eq!(p.border, 3.0);
    assert!(!p.set("pattern", Value::Choice(0)), "not a liquid metal param");
    let mut d = FillParams::from(DitherParams::bayer());
    assert!(d.set("pattern", Value::Choice(3)));
    let FillStyle::Dither(dp) = d.style else { unreachable!() };
    assert_eq!(dp.pattern, DitherPattern::BlueNoise);
}

/// Parse and evaluate the Rust subset emitted by `FillParams::to_rust()`.
fn eval_emitted_rust(code: &str) -> FillParams {
    let mut s = code.strip_prefix("Fx::fill(").and_then(|s| s.strip_suffix(')')).expect(code);
    let mut intensity = 1.0f32;
    if let Some((base, arg)) = s.rsplit_once(".intensity(") {
        intensity = arg.strip_suffix(')').expect(code).parse().expect(code);
        s = base;
    }
    let mut border = 0.0f32;
    if let Some((base, arg)) = s.rsplit_once(".border(") {
        border = arg.strip_suffix(')').expect(code).parse().expect(code);
        s = base;
    }
    let mut fill = if let Some((ty, body)) = s.split_once(" { ") {
        let body = body.strip_suffix(" }").expect(code);
        let (fields_str, preset_rust) = body.rsplit_once(", ..").expect(code);
        let info = fill_styles().iter().find(|i| i.rust == ty.trim()).expect(ty);
        let preset = info.presets.iter().find(|p| p.rust == preset_rust.trim()).expect(preset_rust);
        let mut f = preset.fill();
        // Split top-level `field: expr` pairs outside `[...]` and `(...)`.
        let mut depth = 0i32;
        let mut start = 0usize;
        for (idx, ch) in fields_str.char_indices() {
            match ch {
                '[' | '(' => depth += 1,
                ']' | ')' => depth -= 1,
                ',' if depth == 0 => {
                    apply_field(&mut f, info, &fields_str[start..idx]);
                    start = idx + 1;
                }
                _ => {}
            }
        }
        apply_field(&mut f, info, &fields_str[start..]);
        f
    } else {
        fill_styles().iter().flat_map(|i| i.presets).find(|p| p.rust == s.trim()).expect(s).fill()
    };
    fill.border = border;
    fill.intensity = intensity;
    fill
}

fn parse_hex(s: &str) -> u32 {
    let hex = s.trim().strip_prefix("0x").expect(s);
    assert_eq!(hex.len(), 6, "6-digit hex literal: {s}");
    u32::from_str_radix(hex, 16).expect(s)
}

fn parse_pt(s: &str) -> [f32; 2] {
    let inner = s.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')).expect(s);
    let (x, y) = inner.split_once(',').expect(s);
    [x.trim().parse().expect(s), y.trim().parse().expect(s)]
}

fn apply_field(f: &mut FillParams, info: &StyleInfo, entry: &str) {
    let (id, val) = entry.trim().split_once(": ").expect(entry);
    let param = info.params.iter().find(|p| p.id == id).expect(id);
    let v = match param.kind {
        ParamKind::Number { .. } => {
            assert!(val.contains('.') || val.contains('e'), "float literal has decimal or exp: {val}");
            Value::Number(val.parse().expect(val))
        }
        ParamKind::Count { .. } => Value::Count(val.parse().expect(val)),
        ParamKind::Toggle => Value::Toggle(val.parse().expect(val)),
        ParamKind::Choice(choices) => {
            let idx = choices.iter().position(|c| c.rust == val).expect(val);
            Value::Choice(idx)
        }
        ParamKind::Color => {
            let hex = val.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')).expect(val);
            Value::Color(rgb(parse_hex(hex)))
        }
        ParamKind::Colors { .. } => {
            let list = val.strip_prefix("Stops::hex(&[").and_then(|s| s.strip_suffix("])")).expect(val);
            let hexes: Vec<u32> = list.split(',').map(parse_hex).collect();
            Value::Colors(Stops::hex(&hexes))
        }
        ParamKind::Point => Value::Point(parse_pt(val)),
        ParamKind::Points { .. } => {
            let inner = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')).expect(val);
            let mut pts = [[0.0; 2]; MAX_STOPS];
            let mut k = 0;
            let mut start = 0;
            let mut depth = 0;
            for (i, ch) in inner.char_indices() {
                match ch {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    ',' if depth == 0 => {
                        pts[k] = parse_pt(&inner[start..i]);
                        k += 1;
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            pts[k] = parse_pt(&inner[start..]);
            assert_eq!(k + 1, MAX_STOPS);
            Value::Points(pts)
        }
    };
    assert!(f.set(id, v), "{}.{id}", info.id);
}

#[test]
fn rust_is_written_against_the_closest_preset_and_parses_back() {
    assert_eq!(FillParams::from(LiquidMetalParams::gold()).to_rust(), "Fx::fill(LiquidMetalParams::gold())");
    let f = FillParams::from(LiquidMetalParams { flow: 0.8, ..LiquidMetalParams::rose() }).border(4.0);
    assert_eq!(f.to_rust(), "Fx::fill(LiquidMetalParams { flow: 0.8, ..LiquidMetalParams::rose() }.border(4.0))");
    let d = FillParams::from(DitherParams { pattern: DitherPattern::Bayer8, colors: Stops::hex(&[0x000000, 0xffffff]), ..DitherParams::bayer() });
    assert_eq!(d.to_rust(), "Fx::fill(DitherParams { pattern: DitherPattern::Bayer8, colors: Stops::hex(&[0x000000, 0xffffff]), ..DitherParams::bayer() })");
    for s in fill_styles() {
        for p in s.presets {
            let base = p.fill();
            assert_eq!(base.to_rust(), format!("Fx::fill({})", p.rust));
            assert_eq!(eval_emitted_rust(&base.to_rust()), base);
            let mut all_changed = base.border(3.5).intensity(0.75);
            for param in s.params {
                let alt = match param.kind {
                    ParamKind::Number { min, max, .. } => Value::Number((min + max) * 0.5 + 0.125),
                    ParamKind::Count { max, .. } => Value::Count(max),
                    ParamKind::Toggle => Value::Toggle(!matches!(base.get(param.id), Some(Value::Toggle(true)))),
                    ParamKind::Choice(c) => Value::Choice(c.len() - 1),
                    ParamKind::Color => Value::Color(rgb(0xabcdef)),
                    ParamKind::Colors { min, .. } => Value::Colors(Stops::hex(&[0x102030, 0x405060, 0x708090, 0xa0b0c0, 0xd0e0f0, 0x112233][..min])),
                    ParamKind::Point => Value::Point([0.3, 0.7]),
                    ParamKind::Points { .. } => Value::Points([[0.25, 0.75]; MAX_STOPS]),
                };
                let mut one = base;
                assert!(one.set(param.id, alt));
                assert_eq!(eval_emitted_rust(&one.to_rust()), one, "{}/{}.{}", s.id, p.id, param.id);
                assert!(all_changed.set(param.id, alt));
            }
            assert_eq!(eval_emitted_rust(&all_changed.to_rust()), all_changed, "{}/{} all fields", s.id, p.id);
        }
    }
}

#[test]
fn colours_round_trip_through_hex() {
    for h in [0x000000, 0xffffff, 0xf3c56b, 0x0b3d5c, 0x808080] {
        assert_eq!(rgb(h).hex(), h);
    }
}
