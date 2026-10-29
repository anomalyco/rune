//! The renderer's size-dependent textures and the bind groups that read them.

use super::*;

pub(crate) struct Targets {
    pub(crate) vis_tex: wgpu::Texture,
    pub(crate) gb0: wgpu::TextureView,
    pub(crate) gb1: wgpu::TextureView,
    pub(crate) field: wgpu::TextureView,
    pub(crate) vis: wgpu::TextureView,
    pub(crate) air: wgpu::TextureView,
    pub(crate) field_bg: wgpu::BindGroup,
    pub(crate) shadow_bg: wgpu::BindGroup,
    /// GI's textures, made the first frame GI runs.
    pub(crate) gi: Option<GiTargets>,
}

pub(crate) struct GiTargets {
    pub(crate) scene_emit: wgpu::TextureView,
    pub(crate) scene_occ: wgpu::TextureView,
    /// Merged radiance per cascade level (index = level).
    pub(crate) levels: Vec<wgpu::TextureView>,
    /// Each level's own interval without the merge: black until `inspect` is first on, then written while it's on.
    pub(crate) own: Vec<wgpu::TextureView>,
    pub(crate) fluence: wgpu::TextureView,
    pub(crate) cascade_bgs: Vec<wgpu::BindGroup>,
    pub(crate) own_bgs: Vec<wgpu::BindGroup>,
    pub(crate) fluence_bg: wgpu::BindGroup,
}

impl LightRenderer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn targets(
        device: &wgpu::Device,
        w: u32,
        h: u32,
        fsz: (u32, u32),
        frame: &wgpu::Buffer,
        prims: &wgpu::Buffer,
        lights: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        [field_bgl, shadow_bgl]: [&wgpu::BindGroupLayout; 2],
    ) -> Targets {
        use wgpu::TextureFormat as Tf;
        use wgpu::TextureUsages as U;
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let st = U::STORAGE_BINDING | U::TEXTURE_BINDING | U::COPY_SRC;
        let gb0 = texture(device, "gb0", w, h, 1, Tf::Rgba16Float, rt).create_view(&Default::default());
        let gb1 = texture(device, "gb1", w, h, 1, Tf::R32Uint, rt).create_view(&Default::default());
        let field = texture(device, "field", fsz.0, fsz.1, 1, Tf::Rgba16Float, st).create_view(&Default::default());
        let arr = |label| {
            let t = texture(device, label, fsz.0, fsz.1, 2, Tf::Rgba16Float, st);
            let v = t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            (t, v)
        };
        let (vis_tex, vis) = arr("vis");
        let (_, air) = arr("air");
        let smp = wgpu::BindingResource::Sampler(sampler);
        let field_bg = bind(device, field_bgl, &[frame.as_entire_binding(), prims.as_entire_binding(), tv(&field)]);
        let shadow_bg = bind(device, shadow_bgl, &[frame.as_entire_binding(), lights.as_entire_binding(), tv(&field), smp, tv(&gb0), tv(&vis), tv(&air)]);
        Targets { vis_tex, gb0, gb1, field, vis, air, field_bg, shadow_bg, gi: None }
    }

    /// GI's textures and bind groups, made the first frame GI runs.
    pub(crate) fn gi_targets(&mut self, device: &wgpu::Device) {
        if self.t.gi.is_some() {
            return;
        }
        use wgpu::TextureFormat as Tf;
        let st = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let gsz = self.gi_size;
        let scene_emit = texture(device, "gi emit", gsz.0, gsz.1, 1, Tf::Rgba16Float, st).create_view(&Default::default());
        let scene_occ = texture(device, "gi occ", gsz.0, gsz.1, 1, Tf::Rgba16Float, st).create_view(&Default::default());
        let level = |label| texture(device, label, gsz.0 * 2, gsz.1 * 2, 1, Tf::Rgba16Float, st).create_view(&Default::default());
        let levels: Vec<wgpu::TextureView> = (0..CASCADES).map(|_| level("cascade")).collect();
        let fluence = texture(device, "fluence", gsz.0, gsz.1, 1, Tf::Rgba16Float, st).create_view(&Default::default());
        let (t, black) = (&self.t, &self.black);
        let smp = wgpu::BindingResource::Sampler(&self.sampler);
        // Dispatched top cascade first; each level reads the merged level above it.
        let cascade_bgs = (0..CASCADES as usize)
            .rev()
            .map(|n| {
                let upper = if n + 1 < CASCADES as usize { &levels[n + 1] } else { black };
                bind(
                    device,
                    &self.cascade_bgl,
                    &[self.cascade_u[n].as_entire_binding(), tv(&scene_emit), tv(&scene_occ), tv(&t.field), smp.clone(), tv(upper), tv(&levels[n]), tv(&t.gb0)],
                )
            })
            .collect();
        let fluence_bg = bind(device, &self.fluence_bgl, &[tv(&levels[0]), tv(&fluence)]);
        let own = vec![black.clone(); CASCADES as usize];
        self.t.gi = Some(GiTargets { scene_emit, scene_occ, levels, own, fluence, cascade_bgs, own_bgs: Vec::new(), fluence_bg });
    }

    /// The per-level textures for `inspect`, made the first time it's on with GI.
    pub(crate) fn own_targets(&mut self, device: &wgpu::Device) {
        let Some(g) = self.t.gi.as_ref().filter(|g| g.own_bgs.is_empty()) else { return };
        let st = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let (gw, gh) = (self.gi_size.0 * 2, self.gi_size.1 * 2);
        let own: Vec<wgpu::TextureView> =
            (0..CASCADES).map(|_| texture(device, "cascade own", gw, gh, 1, wgpu::TextureFormat::Rgba16Float, st).create_view(&Default::default())).collect();
        let t = &self.t;
        let smp = wgpu::BindingResource::Sampler(&self.sampler);
        let own_bgs = (0..CASCADES as usize)
            .rev()
            .map(|n| {
                bind(
                    device,
                    &self.cascade_bgl,
                    &[self.own_u[n].as_entire_binding(), tv(&g.scene_emit), tv(&g.scene_occ), tv(&t.field), smp.clone(), tv(&self.black), tv(&own[n]), tv(&t.gb0)],
                )
            })
            .collect();
        if let Some(g) = self.t.gi.as_mut() {
            g.own = own;
            g.own_bgs = own_bgs;
        }
    }

    /// After `prims` was reallocated: rebuild only the bind groups that bind it.
    pub(crate) fn rebind_prims(&mut self, device: &wgpu::Device) {
        self.gbuf_bg = bind(device, &self.gbuf_bgl, &[self.frame.as_entire_binding(), self.prims.as_entire_binding()]);
        self.t.field_bg = bind(device, &self.field_bgl, &[self.frame.as_entire_binding(), self.prims.as_entire_binding(), tv(&self.t.field)]);
    }
}
