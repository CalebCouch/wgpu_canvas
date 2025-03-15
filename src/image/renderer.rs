use wgpu::{PipelineCompilationOptions, BindGroupLayoutDescriptor, RenderPipelineDescriptor, PipelineLayoutDescriptor, TextureViewDimension, BindGroupLayoutEntry, DepthStencilState, TextureSampleType, MultisampleState, BindGroupLayout, RenderPipeline, PrimitiveState, FragmentState, TextureFormat, ShaderStages, BufferUsages, IndexFormat, VertexState, BindingType, RenderPass, BindGroup, Device, Queue, VertexBufferLayout, VertexStepMode, BufferAddress, ShaderModule};
use wgpu_dyn_buffer::{DynamicBufferDescriptor, DynamicBuffer};

use std::collections::HashMap;
use crate::{Area, Shape};
use super::{ImageAtlas, InnerImage, Image};

use crate::shape::{ShapeVertex, RoundedRectangleVertex};

pub struct ImageRenderer {
    bind_group_layout: BindGroupLayout,
    ellipse_renderer: GenericImageRenderer,
    rectangle_renderer: GenericImageRenderer
}

impl ImageRenderer {
    /// Create all unchanging resources here.
    pub fn new(
        device: &Device,
        texture_format: &TextureFormat,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor{
            label: None,
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled: false,
                        view_dimension: TextureViewDimension::D2,
                        sample_type: TextureSampleType::Float{filterable: false},
                    },
                    count: None,
                }
            ]
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("ellipse.wgsl"));
        let ellipse_renderer = GenericImageRenderer::new(device, texture_format, multisample, depth_stencil.clone(), &bind_group_layout, shader, ShapeVertex::layout());
        let shader = device.create_shader_module(wgpu::include_wgsl!("rectangle.wgsl"));
        let rectangle_renderer = GenericImageRenderer::new(device, texture_format, multisample, depth_stencil.clone(), &bind_group_layout, shader, ShapeVertex::layout());
        ImageRenderer{
            bind_group_layout,
            ellipse_renderer,
            rectangle_renderer
        }
    }

    /// Prepare for rendering this frame; create all resources that will be
    /// used during the next render that do not already exist.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        image_atlas: &mut ImageAtlas,
        items: Vec<(Shape, Image, Area)>,
    ) {
        image_atlas.trim_and_bind(queue, device, &self.bind_group_layout);

        let (ellipses, rects, rounded_rects) = items.into_iter().fold(
            (vec![], vec![], vec![]),
            |mut a, (shape, image, area)| {
                let image = image_atlas.get(&image);
                match shape {
                    Shape::Ellipse(stroke, size) => a.0.push((ShapeVertex::new(width, height, area, stroke, size), image)),
                    Shape::Rectangle(stroke, size) => a.1.push((ShapeVertex::new(width, height, area, stroke, size), image)),
                    Shape::RoundedRectangle(stroke, size, corner_radius) =>
                        a.2.push((RoundedRectangleVertex::new(width, height, area, stroke, size, corner_radius), image)),
                }
                a
            }
        );
        self.rectangle_renderer.prepare(device, queue, width, height, rects);
        self.ellipse_renderer.prepare(device, queue, width, height, ellipses);
    }

    /// Render using caller provided render pass.
    pub fn render(&self, render_pass: &mut RenderPass<'_>) {
        self.rectangle_renderer.render(render_pass);
        self.ellipse_renderer.render(render_pass);
    }
}

pub struct GenericImageRenderer {
    render_pipeline: RenderPipeline,
    vertex_buffer: DynamicBuffer,
    index_buffer: DynamicBuffer,
    indices: HashMap<InnerImage, Vec<(u32, u32)>>,
}

impl GenericImageRenderer {
    /// Create all unchanging resources here.
    pub fn new(
        device: &Device,
        texture_format: &TextureFormat,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
        bind_group_layout: &BindGroupLayout,
        shader: ShaderModule,
        vertex_layout: VertexBufferLayout
    ) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor{
            label: None,
            bind_group_layouts: &[bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[vertex_layout]
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState{
                        format: *texture_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })
            //Some((*texture_format).into())],
                ]
            }),
            primitive: PrimitiveState::default(),
            depth_stencil,
            multisample,
            multiview: None,
            cache: None
        });

        let vertex_buffer = DynamicBuffer::new(device, &DynamicBufferDescriptor {
            label: None,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });

        let index_buffer = DynamicBuffer::new(device, &DynamicBufferDescriptor {
            label: None,
            usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
        });

        GenericImageRenderer{
            render_pipeline,
            vertex_buffer,
            index_buffer,
            indices: HashMap::new(),
        }
    }

    /// Prepare for rendering this frame; create all resources that will be
    /// used during the next render that do not already exist.
    pub fn prepare<V: bytemuck::Pod>(
        &mut self,
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        image_vertices: Vec<([V; 4], InnerImage)>,
    ) {
        self.indices.clear();

        let (vertices, indices, indices_buffer) = image_vertices.into_iter().fold(
            (vec![], vec![], HashMap::<InnerImage, Vec<(u32, u32)>>::new()),
            |mut a, (vertices, image)| {
                let start = a.1.len();

                let l = a.0.len() as u16;
                a.0.extend(vertices);
                a.1.extend([l, l+1, l+2, l+1, l+2, l+3]);

                let index = (start as u32, a.1.len() as u32);
                match a.2.get_mut(&image) {
                    Some(indices) => indices.push(index),
                    None => {a.2.insert(image, vec![index]);}
                }
                a
            }
        );

        self.indices = indices_buffer;
        self.vertex_buffer.write_buffer(device, queue, bytemuck::cast_slice(&vertices));
        self.index_buffer.write_buffer(device, queue, bytemuck::cast_slice(&indices));
    }

    /// Render using caller provided render pass.
    pub fn render(&self, render_pass: &mut RenderPass<'_>) {
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.as_ref().slice(..));
        render_pass.set_index_buffer(self.index_buffer.as_ref().slice(..), IndexFormat::Uint16);
        for (bind_group, indices) in &self.indices {
            render_pass.set_bind_group(0, Some(&**bind_group), &[]);
            for (start, end) in indices {
                render_pass.draw_indexed(*start..*end, 0, 0..1);
            }
        }
    }
}
