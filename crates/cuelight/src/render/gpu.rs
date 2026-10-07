//! The resolved layers drawn with `vello_gpu`, the renderer vello is
//! moving to, beside the classic one, so the two can be compared frame
//! for frame (#293). Offscreen and at the show's own size only: what a
//! comparison needs, not yet what a player needs.

use super::{
    background_color, bez_path, blend_mode, gradient_brush, ImageCache, RenderError, RgbaFrame,
};
use crate::engine::{Engine, ResolvedLayer, ResolvedShape};
use cuelight_core::Error;
use std::collections::HashMap;
use std::sync::Arc;
use vello::kurbo::{Affine, Circle, Join, Rect, Shape, Stroke};
use vello::peniko::{Brush, Color, Extend, ImageQuality};
use vello::wgpu;
use vello_gpu::{Image, ImageId, ImageSource, PaintType, PixelMetadata, Pixmap, Tint, TintMode};

/// How finely curves are flattened where `vello_gpu` takes a path for a
/// shape, as the classic renderer does internally.
const TOLERANCE: f64 = 0.1;

/// Renders an engine's frames offscreen with `vello_gpu`, at the show's
/// own size, read back as RGBA8: [`super::Renderer::render_to_rgba`] for
/// the other renderer.
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::AdapterInfo,
    /// The image cache the classic renderer keeps, reused for what it
    /// prepares (downscaled images, sheet cells, text rasters, fonts).
    images: ImageCache,
    /// Pixels already in `vello_gpu`'s atlas, by the blob they came from.
    uploaded: HashMap<u64, ImageId>,
    target: Option<Target>,
}

/// What a frame of one size is drawn into and read back from.
struct Target {
    size: [u16; 2],
    renderer: vello_gpu::Renderer,
    resources: vello_gpu::Resources,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    buffer: wgpu::Buffer,
    bytes_per_row: u32,
}

impl GpuRenderer {
    /// A renderer on the first adapter there is.
    pub fn new() -> Result<Self, RenderError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|_| RenderError::NoAdapter)?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| RenderError::Device(e.to_string()))?;
        Ok(Self {
            device,
            queue,
            adapter: info,
            images: ImageCache::new(),
            uploaded: HashMap::new(),
            target: None,
        })
    }

    pub fn adapter(&self) -> &wgpu::AdapterInfo {
        &self.adapter
    }

    /// The engine's current frame at the show's size, its output mode
    /// applied after reading back, as the classic offscreen renderer does.
    pub fn render_to_rgba(&mut self, engine: &Engine) -> Result<RgbaFrame, RenderError> {
        let show = engine.show().ok_or(Error::NoShow)?;
        let size = show.size.map(|n| u16::try_from(n).unwrap_or(u16::MAX));
        let items = engine.resolved_layers()?;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(Target::new(&self.device, size));
            // A new renderer has an empty atlas.
            self.uploaded.clear();
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cuelight-gpu"),
            });
        let mut scene = vello_gpu::Scene::new(size[0], size[1]);
        let background = background_color(engine);
        // The background, as the classic renderer's base color.
        scene.set_paint(background);
        scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(size[0]), f64::from(size[1])));
        let Some(target) = self.target.as_mut() else {
            return Err(RenderError::Unrenderable("no target".to_owned()));
        };
        let mut draw = Draw {
            engine,
            images: &mut self.images,
            uploaded: &mut self.uploaded,
            device: &self.device,
            queue: &self.queue,
            encoder: &mut encoder,
            target,
        };
        for item in items {
            draw.item(&mut scene, item);
        }
        let render_size = vello_gpu::RenderSize {
            width: size[0],
            height: size[1],
        };
        target
            .renderer
            .render(
                &scene,
                &mut target.resources,
                &self.device,
                &self.queue,
                &mut encoder,
                &render_size,
                &target.view,
                Some(&target.depth),
                &vello_gpu::TextureBindings::new(),
                vello_gpu::TargetInit::Clear(vello_gpu::ClearSettings::default()),
            )
            .map_err(|e| RenderError::Vello(format!("{e:?}")))?;
        let mut frame = target.read_back(&self.device, &self.queue, encoder)?;
        engine.output().apply(&mut frame.pixels);
        Ok(frame)
    }
}

impl Target {
    fn new(device: &wgpu::Device, size: [u16; 2]) -> Self {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let (renderer, resources) = vello_gpu::Renderer::new(
            device,
            &vello_gpu::RenderTargetConfig {
                format,
                width: size[0],
                height: size[1],
            },
        );
        let extent = wgpu::Extent3d {
            width: u32::from(size[0]),
            height: u32::from(size[1]),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cuelight-gpu-target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = vello_gpu::Renderer::create_depth_texture_view(
            device,
            &vello_gpu::RenderSize {
                width: size[0],
                height: size[1],
            },
        );
        let bytes_per_row =
            (u32::from(size[0]) * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cuelight-gpu-readback"),
            size: u64::from(bytes_per_row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            size,
            renderer,
            resources,
            texture,
            view,
            depth,
            buffer,
            bytes_per_row,
        }
    }

    fn read_back(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut encoder: wgpu::CommandEncoder,
    ) -> Result<RgbaFrame, RenderError> {
        let [width, height] = self.size.map(u32::from);
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(format!("{e:?}")))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * self.bytes_per_row) as usize;
            let line = mapped
                .get(start..start + (width * 4) as usize)
                .ok_or_else(|| RenderError::Readback("the frame came back short".to_owned()))?;
            pixels.extend_from_slice(line);
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(RgbaFrame {
            width,
            height,
            pixels,
        })
    }
}

/// One frame being drawn: where its images come from and go.
struct Draw<'a> {
    engine: &'a Engine,
    images: &'a mut ImageCache,
    uploaded: &'a mut HashMap<u64, ImageId>,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    encoder: &'a mut wgpu::CommandEncoder,
    target: &'a mut Target,
}

impl Draw<'_> {
    /// The atlas id of prepared pixels, uploaded the first time they are
    /// drawn.
    fn image(&mut self, data: &vello::peniko::ImageData) -> ImageId {
        let key = data.data.id();
        if let Some(id) = self.uploaded.get(&key) {
            return *id;
        }
        let pixmap = Arc::new(Pixmap::from_parts(
            data.data.data().to_vec(),
            u16::try_from(data.width).unwrap_or(u16::MAX),
            u16::try_from(data.height).unwrap_or(u16::MAX),
            PixelMetadata {
                may_have_transparency: true,
                alpha_type: data.alpha_type,
            },
        ));
        let id = self.target.renderer.upload_image(
            &mut self.target.resources,
            self.device,
            self.queue,
            self.encoder,
            &pixmap,
        );
        self.uploaded.insert(key, id);
        id
    }

    fn item(&mut self, scene: &mut vello_gpu::Scene, layer: ResolvedLayer) {
        let [r, g, b, a] = layer.color;
        let alpha = (f64::from(a) / 255.0 * layer.opacity).clamp(0.0, 1.0);
        let color = Color::from_rgba8(r, g, b, (alpha * 255.0).round() as u8);
        let paint: PaintType = match layer
            .gradient
            .as_ref()
            .map(|g| gradient_brush(g, layer.opacity))
        {
            Some(Brush::Gradient(gradient)) => PaintType::Gradient(gradient),
            _ => PaintType::Solid(color),
        };
        let brush_space = layer
            .gradient
            .as_ref()
            .map(|g| g.space)
            .filter(|space| *space != crate::Transform::IDENTITY)
            .map(|space| Affine::new(space.0));
        let placement = Affine::new(layer.transform.0);
        scene.set_transform(placement);
        scene.reset_paint_transform();
        scene.reset_tint();
        // A blended item blends as it is drawn, straight onto what is
        // beneath: one shape blended is the same picture as one shape in a
        // layer of its own blended, without the layer, and a layer each
        // for a hundred lamps costs the GPU a pass each. A blended group
        // is still a layer, since it is composited as one picture.
        let blended = None;
        scene.set_blend_mode(blend_mode(layer.blend).unwrap_or_default());
        match layer.shape {
            ResolvedShape::BlendBegin { blend } => {
                scene.reset_transform();
                if let Some(mode) = blend_mode(blend) {
                    scene.push_blend_layer(mode);
                } else {
                    scene.push_opacity_layer(1.0);
                }
            }
            ResolvedShape::BlendEnd => scene.pop_layer(),
            // A clip that only clips, not a layer of its own: what is
            // drawn inside it still blends with what is beneath, as it
            // does in the classic renderer. A clip layer would draw it on
            // nothing first, and a multiply would have nothing to darken.
            ResolvedShape::ClipBegin { shape } => match clip_path(&shape) {
                Some(path) => scene.push_clip_path(&path),
                // Whatever is not a shape clips nothing, but a clip is
                // pushed all the same so the end balances.
                None => scene.push_clip_rect(&Rect::new(-1e9, -1e9, 1e9, 1e9)),
            },
            ResolvedShape::ClipEnd => scene.pop_clip(),
            ResolvedShape::Rect {
                x,
                y,
                width,
                height,
            } => {
                fill(scene, paint, brush_space);
                scene.fill_rect(&Rect::new(x, y, x + width, y + height));
            }
            ResolvedShape::Circle { cx, cy, radius } => {
                fill(scene, paint, brush_space);
                scene.fill_path(&Circle::new((cx, cy), radius).to_path(TOLERANCE));
            }
            ResolvedShape::Polygon { points } => {
                let mut path = vello::kurbo::BezPath::new();
                for (i, &[x, y]) in points.iter().enumerate() {
                    if i == 0 {
                        path.move_to((x, y));
                    } else {
                        path.line_to((x, y));
                    }
                }
                path.close_path();
                fill(scene, paint, brush_space);
                scene.fill_path(&path);
            }
            ResolvedShape::Path {
                elements,
                stroke,
                stroke_space,
            } => {
                let path = bez_path(&elements);
                if layer.color[3] > 0 || layer.gradient.is_some() {
                    fill(scene, paint, brush_space);
                    scene.fill_path(&path);
                }
                if let Some(([r, g, b, a], width)) = stroke {
                    let alpha = (f64::from(a) / 255.0 * layer.opacity).clamp(0.0, 1.0);
                    scene.reset_paint_transform();
                    scene.set_paint(Color::from_rgba8(r, g, b, (alpha * 255.0).round() as u8));
                    scene.set_stroke(Stroke::new(width));
                    match stroke_space.map(|space| (space, space.invert())) {
                        None => scene.stroke_path(&path),
                        Some((space, Some(back))) => {
                            scene.set_transform(placement * Affine::new(space.0));
                            scene.stroke_path(&(Affine::new(back.0) * path));
                        }
                        Some((_, None)) => {}
                    }
                }
            }
            ResolvedShape::GlyphRun {
                font,
                size,
                glyphs,
                border,
                ..
            } => {
                let font = self.images.font(&font);
                let run = || {
                    glyphs.iter().map(|g| glifo::Glyph {
                        id: g.id,
                        x: g.x as f32,
                        y: g.y as f32,
                    })
                };
                if let Some(([r, g, b, a], width)) = border {
                    let alpha = (f64::from(a) / 255.0 * layer.opacity).clamp(0.0, 1.0);
                    scene.set_paint(Color::from_rgba8(r, g, b, (alpha * 255.0).round() as u8));
                    scene.set_stroke(Stroke::new(width * 2.0).with_join(Join::Round));
                    let _ = scene
                        .glyph_run(&mut self.target.resources, &font)
                        .font_size(size as f32)
                        .atlas_cache(true)
                        .stroke_glyphs(run());
                }
                scene.set_paint(color);
                let _ = scene
                    .glyph_run(&mut self.target.resources, &font)
                    .font_size(size as f32)
                    .atlas_cache(true)
                    .fill_glyphs(run());
            }
            ResolvedShape::Image {
                image,
                source,
                x,
                y,
                width,
                height,
                tile,
                nearest,
            } => {
                let Some(data) = self.engine.image(&image) else {
                    return close(scene, blended);
                };
                let (tile_w, tile_h, ox, oy) = match tile {
                    None => (width, height, 0.0, 0.0),
                    Some(tile) => (tile.width, tile.height, tile.offset[0], tile.offset[1]),
                };
                let [a, b, ..] = placement.as_coeffs();
                let onto = match self.engine.pixel_grid() || nearest {
                    true => f64::MAX,
                    false => tile_w * (a * a + b * b).sqrt(),
                };
                let pixels = self.images.fitted(&image, data, source, onto);
                let (pw, ph) = (f64::from(pixels.width), f64::from(pixels.height));
                let id = self.image(&pixels);
                let transform = placement
                    * Affine::translate((x + ox, y + oy))
                    * Affine::scale_non_uniform(tile_w / pw, tile_h / ph);
                let mut brush = atlas(id).with_alpha(layer.opacity as f32);
                if nearest {
                    brush = brush.with_quality(ImageQuality::Low);
                }
                if tile.is_some() {
                    brush = brush.with_extend(Extend::Repeat);
                }
                if layer.color != [255; 4] {
                    let [r, g, b, a] = layer.color;
                    scene.set_tint(Some(Tint {
                        color: Color::from_rgba8(r, g, b, a),
                        mode: TintMode::Multiply,
                    }));
                }
                scene.set_paint(brush);
                match tile {
                    Some(_) => {
                        scene.set_paint_transform(placement.inverse() * transform);
                        scene.fill_rect(&Rect::new(x, y, x + width, y + height));
                    }
                    None => {
                        scene.set_transform(transform);
                        scene.fill_rect(&Rect::new(0.0, 0.0, pw, ph));
                    }
                }
            }
            ResolvedShape::Bitmap {
                image,
                x,
                y,
                width,
                height,
            } => {
                let pixels = self.images.bitmap(&image);
                let id = self.image(&pixels);
                let (pw, ph) = (f64::from(image.width), f64::from(image.height));
                scene.set_transform(
                    placement
                        * Affine::translate((x, y))
                        * Affine::scale_non_uniform(width / pw, height / ph),
                );
                scene.set_paint(atlas(id).with_alpha(layer.opacity as f32));
                scene.fill_rect(&Rect::new(0.0, 0.0, pw, ph));
            }
        }
        close(scene, blended);
    }
}

/// A brush drawing pixels already in the atlas.
fn atlas(id: ImageId) -> Image {
    Image {
        image: ImageSource::opaque_id(id),
        sampler: vello::peniko::ImageSampler::default(),
    }
}

/// Pop the layer a blended item was drawn into.
fn close(scene: &mut vello_gpu::Scene, blended: Option<vello::peniko::BlendMode>) {
    if blended.is_some() {
        scene.pop_layer();
    }
}

/// Set a fill's paint and where a gradient's own coordinates sit.
fn fill(scene: &mut vello_gpu::Scene, paint: PaintType, brush_space: Option<Affine>) {
    match brush_space {
        Some(space) => scene.set_paint_transform(space),
        None => scene.reset_paint_transform(),
    }
    scene.set_paint(paint);
}

/// A clip's shape as the path `vello_gpu` clips to.
fn clip_path(shape: &ResolvedShape) -> Option<vello::kurbo::BezPath> {
    Some(match *shape {
        ResolvedShape::Rect {
            x,
            y,
            width,
            height,
        } => Rect::new(x, y, x + width, y + height).to_path(TOLERANCE),
        ResolvedShape::Circle { cx, cy, radius } => {
            Circle::new((cx, cy), radius).to_path(TOLERANCE)
        }
        ResolvedShape::Path { ref elements, .. } => bez_path(elements),
        _ => return None,
    })
}
