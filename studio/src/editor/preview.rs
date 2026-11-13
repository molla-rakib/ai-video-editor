//! The preview: draws the evaluated scene with the compositor inside iced's
//! own render pass. The canvas can be zoomed past the pane, so it is drawn
//! into the visible part of its rectangle only.

use std::sync::Arc;

use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;

use crate::render::compositor::Compositor;
use crate::render::scene::Scene;

pub struct Preview {
    pub scene: Arc<Scene>,
    /// Where the canvas sits, relative to the widget (logical pixels). It may extend past the widget.
    pub canvas: Rectangle,
}

impl<Message> shader::Program<Message> for Preview {
    type State = ();
    type Primitive = PreviewPrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> PreviewPrimitive {
        PreviewPrimitive { scene: self.scene.clone(), canvas: self.canvas }
    }
}

#[derive(Debug)]
pub struct PreviewPrimitive {
    scene: Arc<Scene>,
    canvas: Rectangle,
}

pub struct PreviewPipeline {
    compositor: Compositor,
    /// Visible part of the canvas in physical pixels (x, y, w, h); empty when off screen.
    visible: [f32; 4],
}

impl shader::Pipeline for PreviewPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        PreviewPipeline { compositor: Compositor::new(device, queue, format), visible: [0.0; 4] }
    }
}

impl shader::Primitive for PreviewPrimitive {
    type Pipeline = PreviewPipeline;

    fn prepare(&self, pipeline: &mut PreviewPipeline, device: &wgpu::Device, queue: &wgpu::Queue, bounds: &Rectangle, viewport: &Viewport) {
        let scale = viewport.scale_factor() as f32;
        let (cw, ch) = (self.scene.width as f32, self.scene.height as f32);
        let target = viewport.physical_size();
        // The canvas rectangle and the widget, in physical pixels.
        let r = [((bounds.x + self.canvas.x) * scale).round(), ((bounds.y + self.canvas.y) * scale).round(), (self.canvas.width * scale).round().max(1.0), (self.canvas.height * scale).round().max(1.0)];
        let (bx0, by0) = ((bounds.x * scale).round().max(0.0), (bounds.y * scale).round().max(0.0));
        let (bx1, by1) = (((bounds.x + bounds.width) * scale).round().min(target.width as f32), ((bounds.y + bounds.height) * scale).round().min(target.height as f32));
        let (x0, y0) = (r[0].max(bx0), r[1].max(by0));
        let (x1, y1) = ((r[0] + r[2]).min(bx1), (r[1] + r[3]).min(by1));
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            pipeline.visible = [0.0; 4];
            return;
        }
        let v = [x0, y0, x1 - x0, y1 - y0];
        pipeline.visible = v;
        // Canvas pixels → clip space of the visible rectangle.
        let (kx, ky) = (r[2] / cw, r[3] / ch);
        let view = [kx * 2.0 / v[2], (r[0] - v[0]) * 2.0 / v[2] - 1.0, -ky * 2.0 / v[3], 1.0 - (r[1] - v[1]) * 2.0 / v[3]];
        let aa = cw / r[2];
        pipeline.compositor.prepare(device, queue, &self.scene, aa, view);
    }

    fn draw(&self, pipeline: &PreviewPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let [x, y, w, h] = pipeline.visible;
        if w < 1.0 || h < 1.0 {
            return true;
        }
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
        pipeline.compositor.draw(pass);
        true
    }
}
