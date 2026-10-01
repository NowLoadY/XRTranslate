//! Mesh instances, materials and expression weights, independent of the GPU backend.
use eframe::egui::Color32;
use glam::Mat4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    Body,
    Eye,
    Mouth,
    Cap,
    Visor,
    Scarf,
    Tail,
}
impl Geometry {
    pub const ALL: [Self; 7] = [
        Self::Body,
        Self::Eye,
        Self::Mouth,
        Self::Cap,
        Self::Visor,
        Self::Scarf,
        Self::Tail,
    ];
}

#[derive(Clone)]
pub struct Part {
    pub geometry: Geometry,
    pub transform: Mat4,
    pub color: Color32,
    pub morph: [f32; 2],
    pub material: Material,
}

#[derive(Clone, Copy)]
pub struct Material {
    pub shade_contrast: f32,
    pub outline_width: f32,
}
impl Material {
    pub const CLAY: Self = Self {
        shade_contrast: 0.26,
        outline_width: 0.014,
    };
    pub const FABRIC: Self = Self {
        shade_contrast: 0.34,
        outline_width: 0.012,
    };
}

impl Part {
    pub fn new(geometry: Geometry, color: Color32) -> Self {
        Self {
            geometry,
            transform: Mat4::IDENTITY,
            color,
            morph: [0.0; 2],
            material: Material::CLAY,
        }
    }
}

#[derive(Clone, Default)]
pub struct Model {
    pub parts: Vec<Part>,
}

impl Model {
    pub fn add(&mut self, part: Part) {
        self.parts.push(part);
    }
}
