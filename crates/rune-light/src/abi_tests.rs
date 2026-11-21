use crate::glass::{Glass, GlassLight, GlassU};
use crate::{Cascade, FLAG_EMISSIVE, FLAG_GI_HINT, FLAG_OCCLUDER, FLAG_RAISED, FrameU, Light, Prim};

#[test]
fn wgsl_uniform_storage_and_flags_match_rust() {
    use wgpu::naga::{Expression, Literal, Module, TypeInner, front::wgsl, proc::Layouter, valid::{Capabilities, ValidationFlags, Validator}};

    let parse = |label: &str, source: &str| -> (Module, Layouter) {
        let module = wgsl::parse_str(source).unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let mut layouter = Layouter::default();
        layouter.update(module.to_ctx()).unwrap_or_else(|e| panic!("{label}: {e}"));
        (module, layouter)
    };

    let check_struct = |label: &str, module: &Module, layouter: &Layouter, name: &str, size: usize, expected: &[(&str, usize)]| {
        let (handle, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{label}: WGSL struct {name} not found"));
        assert_eq!(layouter[handle].size as usize, size, "{label} {name} size");
        let TypeInner::Struct { members, span } = &ty.inner else { panic!("{label}: {name} is not a struct") };
        assert_eq!(*span as usize, size, "{label} {name} span");
        assert_eq!(members.len(), expected.len(), "{label} {name} member count");
        for (member, &(field, offset)) in members.iter().zip(expected) {
            assert_eq!(member.name.as_deref(), Some(field), "{label} {name}.{field} name");
            assert_eq!(member.offset as usize, offset, "{label} {name}.{field} offset");
        }
    };

    let check_u32_const = |label: &str, module: &Module, name: &str, expected: u32| {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{label}: WGSL const {name} not found"));
        match module.global_expressions[c.init] {
            Expression::Literal(Literal::U32(val)) => assert_eq!(val, expected, "{label} const {name}"),
            ref other => panic!("{label}: const {name} is {other:?}, expected u32 literal"),
        }
    };

    let gbuf_src = format!("{}\n{}", crate::gpu::COMMON, include_str!("shaders/gbuffer.wgsl"));
    let (gbuf_mod, gbuf_layout) = parse("common+gbuffer.wgsl", &gbuf_src);
    check_struct(
        "common.wgsl",
        &gbuf_mod,
        &gbuf_layout,
        "Frame",
        std::mem::size_of::<FrameU>(),
        &[
            ("sf", std::mem::offset_of!(FrameU, sf)),
            ("gs", std::mem::offset_of!(FrameU, gs)),
            ("ambient", std::mem::offset_of!(FrameU, ambient)),
            ("counts", std::mem::offset_of!(FrameU, counts)),
            ("misc", std::mem::offset_of!(FrameU, misc)),
            ("text", std::mem::offset_of!(FrameU, text)),
            ("gi", std::mem::offset_of!(FrameU, gi)),
            ("bg", std::mem::offset_of!(FrameU, bg)),
            ("ext", std::mem::offset_of!(FrameU, ext)),
        ],
    );
    check_struct(
        "common.wgsl",
        &gbuf_mod,
        &gbuf_layout,
        "Prim",
        std::mem::size_of::<Prim>(),
        &[
            ("rect", std::mem::offset_of!(Prim, rect)),
            ("radii", std::mem::offset_of!(Prim, radii)),
            ("z", std::mem::offset_of!(Prim, z)),
            ("mat", std::mem::offset_of!(Prim, mat)),
            ("emit", std::mem::offset_of!(Prim, emit)),
            ("albedo", std::mem::offset_of!(Prim, albedo)),
        ],
    );
    check_struct(
        "common.wgsl",
        &gbuf_mod,
        &gbuf_layout,
        "Light",
        std::mem::size_of::<Light>(),
        &[
            ("pos", std::mem::offset_of!(Light, pos)),
            ("color", std::mem::offset_of!(Light, color)),
            ("axis", std::mem::offset_of!(Light, axis)),
            ("extra", std::mem::offset_of!(Light, extra)),
        ],
    );
    check_u32_const("common.wgsl", &gbuf_mod, "F_OCC", FLAG_OCCLUDER);
    check_u32_const("common.wgsl", &gbuf_mod, "F_EMIT", FLAG_EMISSIVE);
    check_u32_const("common.wgsl", &gbuf_mod, "F_RAISED", FLAG_RAISED);
    check_u32_const("common.wgsl", &gbuf_mod, "F_HINT", FLAG_GI_HINT);

    let gi_src = format!("{}\n{}", crate::gpu::COMMON, crate::gpu::gi_section("cascades"));
    let (gi_mod, gi_layout) = parse("common+gi(cascades).wgsl", &gi_src);
    check_struct(
        "gi.wgsl",
        &gi_mod,
        &gi_layout,
        "Cascade",
        std::mem::size_of::<Cascade>(),
        &[
            ("n", std::mem::offset_of!(Cascade, n)),
            ("count", std::mem::offset_of!(Cascade, count)),
            ("merge", std::mem::offset_of!(Cascade, merge)),
            ("pad1", std::mem::offset_of!(Cascade, pad1)),
        ],
    );

    let glass_src = crate::glass::section("glass");
    let (glass_mod, glass_layout) = parse("glass.wgsl", &glass_src);
    check_struct(
        "glass.wgsl",
        &glass_mod,
        &glass_layout,
        "U",
        std::mem::size_of::<GlassU>(),
        &[
            ("size", std::mem::offset_of!(GlassU, size)),
            ("misc", std::mem::offset_of!(GlassU, misc)),
            ("shadow", std::mem::offset_of!(GlassU, shadow)),
        ],
    );
    check_struct(
        "glass.wgsl",
        &glass_mod,
        &glass_layout,
        "Glass",
        std::mem::size_of::<Glass>(),
        &[
            ("a", std::mem::offset_of!(Glass, a)),
            ("b", std::mem::offset_of!(Glass, b)),
            ("r", std::mem::offset_of!(Glass, r)),
            ("p0", std::mem::offset_of!(Glass, p0)),
            ("p1", std::mem::offset_of!(Glass, p1)),
            ("tint", std::mem::offset_of!(Glass, tint)),
        ],
    );
    check_struct(
        "glass.wgsl",
        &glass_mod,
        &glass_layout,
        "GLight",
        std::mem::size_of::<GlassLight>(),
        &[
            ("pos", std::mem::offset_of!(GlassLight, pos)),
            ("color", std::mem::offset_of!(GlassLight, color)),
        ],
    );
}
