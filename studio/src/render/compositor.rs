//! Draws a `Scene` with wgpu. Works on any device: iced's for the preview, a
//! headless one for export.

use std::collections::HashMap;

use iced::wgpu;

use super::scene::{LayerSpec, Scene};

/// Uniforms per layer, padded to the dynamic-offset alignment.
const STRIDE: u64 = 512;
const UNIFORM_SIZE: u64 = 496;

struct Slot {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    version: u64,
    buffer_generation: u64,
    last_used: u64,
}

pub struct Compositor {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    capacity: u64,
    buffer_generation: u64,
    white: Slot,
    slots: HashMap<u64, Slot>,
    draws: Vec<(Option<u64>, u32)>,
    srgb_target: bool,
    frame_no: u64,
}

impl Compositor {
    /// The `view` that fills a whole render target with the canvas.
    pub fn full_view(width: u32, height: u32) -> [f32; 4] {
        [2.0 / width.max(1) as f32, -1.0, -2.0 / height.max(1) as f32, 1.0]
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Compositor {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("layer"), source: wgpu::ShaderSource::Wgsl(include_str!("layer.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("layer"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let capacity = 32;
        let uniforms = uniform_buffer(device, capacity);
        let white_tex = texture(device, 1, 1);
        upload(queue, &white_tex, 1, 1, &[255, 255, 255, 255]);
        let white_bg = bind_group(device, &layout, &uniforms, &white_tex, &sampler);
        Compositor {
            pipeline,
            layout,
            sampler,
            uniforms,
            capacity,
            buffer_generation: 0,
            white: Slot { texture: white_tex, bind_group: white_bg, size: (1, 1), version: 0, buffer_generation: 0, last_used: 0 },
            slots: HashMap::new(),
            draws: Vec::new(),
            srgb_target: format.is_srgb(),
            frame_no: 0,
        }
    }

    /// Uploads textures and per-layer uniforms. `aa` is canvas pixels per screen pixel.
    /// `view` maps canvas pixels to clip space: x * view[0] + view[1], y * view[2] + view[3].
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene, aa: f32, view: [f32; 4]) {
        self.frame_no += 1;
        let count = scene.layers.len() as u64 + 1;
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.uniforms = uniform_buffer(device, self.capacity);
            self.buffer_generation += 1;
        }
        if self.white.buffer_generation != self.buffer_generation {
            self.white.bind_group = bind_group(device, &self.layout, &self.uniforms, &self.white.texture, &self.sampler);
            self.white.buffer_generation = self.buffer_generation;
        }

        let (w, h) = (scene.width as f32, scene.height as f32);
        let background = LayerSpec {
            clip: 0,
            slot: 0,
            frame: None,
            center: [w / 2.0, h / 2.0],
            size: [w, h],
            rotation: 0.0,
            opacity: 1.0,
            radius: 0.0,
            border: 0.0,
            border_color: [0.0; 4],
            shadow_blur: 0.0,
            shadow_y: 0.0,
            shadow_color: [0.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            tint_top: scene.background,
            tint_bottom: scene.background,
            grade: [0.0; 4],
            grade2: [0.0; 4],
            grade3: [0.0; 4],
            grade4: [0.0; 4],
            shape: [0.0, 0.0, 0.0, std::f32::consts::PI],
            blurs: [[0.0; 8]; super::scene::MAX_BLURS],
            masks: [[0.0; 8]; super::scene::MAX_MASKS],
            blurrable: false,
            blur_box: None,
            mask_box: None,
        };
        let mut staging = vec![0u8; (count * STRIDE) as usize];
        self.draws.clear();
        for (i, layer) in std::iter::once(&background).chain(scene.layers.iter()).enumerate() {
            let has_texture = layer.frame.is_some();
            let key = layer.frame.as_ref().map(|f| {
                let gen = self.buffer_generation;
                let slot = self.slots.entry(layer.slot).or_insert_with(|| {
                    let tex = texture(device, f.width, f.height);
                    let bg = bind_group(device, &self.layout, &self.uniforms, &tex, &self.sampler);
                    Slot { texture: tex, bind_group: bg, size: (f.width, f.height), version: 0, buffer_generation: gen, last_used: 0 }
                });
                if slot.size != (f.width, f.height) {
                    slot.texture = texture(device, f.width, f.height);
                    slot.size = (f.width, f.height);
                    slot.version = 0;
                    slot.buffer_generation = u64::MAX;
                }
                if slot.buffer_generation != gen {
                    slot.bind_group = bind_group(device, &self.layout, &self.uniforms, &slot.texture, &self.sampler);
                    slot.buffer_generation = gen;
                }
                if slot.version != f.version {
                    upload(queue, &slot.texture, f.width, f.height, &f.rgba);
                    slot.version = f.version;
                }
                slot.last_used = self.frame_no;
                layer.slot
            });
            let pad = if layer.shadow_color[3] > 0.0 { layer.shadow_blur * 2.0 + layer.shadow_y.abs() + 2.0 } else { 2.0 * aa };
            let mut u = [0.0f32; 124];
            u[..60].copy_from_slice(&[
                layer.center[0], layer.center[1], layer.size[0] / 2.0, layer.size[1] / 2.0,
                layer.rotation, layer.opacity, layer.radius, layer.border,
                layer.shadow_blur, layer.shadow_y, pad, aa,
                layer.uv[0], layer.uv[1], layer.uv[2], layer.uv[3],
                layer.tint_top[0], layer.tint_top[1], layer.tint_top[2], layer.tint_top[3],
                layer.tint_bottom[0], layer.tint_bottom[1], layer.tint_bottom[2], layer.tint_bottom[3],
                layer.border_color[0], layer.border_color[1], layer.border_color[2], layer.border_color[3],
                layer.shadow_color[0], layer.shadow_color[1], layer.shadow_color[2], layer.shadow_color[3],
                layer.grade[0], layer.grade[1], layer.grade[2], layer.grade[3],
                layer.grade2[0], layer.grade2[1], layer.grade2[2], layer.grade2[3],
                layer.grade3[0], layer.grade3[1], layer.grade3[2], layer.grade3[3],
                layer.grade4[0], layer.grade4[1], layer.grade4[2], layer.grade4[3],
                w, h, if self.srgb_target { 1.0 } else { 0.0 }, if has_texture { 1.0 } else { 0.0 },
                layer.shape[0], layer.shape[1], layer.shape[2], layer.shape[3],
                view[0], view[1], view[2], view[3],
            ]);
            for (i, b) in layer.blurs.iter().enumerate() {
                // The shader keeps the four rects, then the four parameter sets.
                u[60 + i * 4..64 + i * 4].copy_from_slice(&b[..4]);
                u[76 + i * 4..80 + i * 4].copy_from_slice(&b[4..]);
            }
            for (i, m) in layer.masks.iter().enumerate() {
                u[92 + i * 4..96 + i * 4].copy_from_slice(&m[..4]);
                u[108 + i * 4..112 + i * 4].copy_from_slice(&m[4..]);
            }
            let offset = i as u64 * STRIDE;
            staging[offset as usize..(offset + UNIFORM_SIZE) as usize].copy_from_slice(bytemuck::cast_slice(&u));
            self.draws.push((key, offset as u32));
        }
        queue.write_buffer(&self.uniforms, 0, &staging);
        // Free textures of clips that left the screen a while ago.
        let now = self.frame_no;
        self.slots.retain(|_, s| now - s.last_used < 240);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        for (key, offset) in &self.draws {
            let bg = key.and_then(|k| self.slots.get(&k)).map(|s| &s.bind_group).unwrap_or(&self.white.bind_group);
            pass.set_bind_group(0, bg, &[*offset]);
            pass.draw(0..4, 0..1);
        }
    }
}

fn uniform_buffer(device: &wgpu::Device, layers: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some("layer uniforms"), size: layers * STRIDE, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

fn texture(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("layer"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload(queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32, rgba: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, tex: &wgpu::Texture, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    let view = tex.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("layer"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: uniforms, offset: 0, size: wgpu::BufferSize::new(UNIFORM_SIZE) }) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}
