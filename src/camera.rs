use nalgebra::{Matrix3, Vector2, Vector3};
use std::ops::Mul;

pub struct Camera {
    position: Vector2<f32>,
    rotation: f32,
    zoom_raw: f32,
    zoom_vis: f32,
    perspective_correction: bool,
}

impl Camera {
    // We're not doing any scaling on the viewport so 2 u/s would span edge to edge in a single second
    const MOVE_SPEED_PER_S: f32 = 0.5;

    // Radians
    const ROT_SPEED_PER_S: f32 = 1.0;

    // Idk
    const ZOOM_SPEED_PER_S: f32 = 1.0;

    pub fn new(perspective_correction: bool) -> Camera {
        Camera {
            position: Vector2::new(0.0, 0.0),
            rotation: 0.0,
            zoom_raw: 0.0,
            zoom_vis: 1.0,
            perspective_correction,
        }
    }

    /// Affected by zoom level
    pub fn translate(&mut self, amount: Vector2<f32>) {
        let rot = Matrix3::new_rotation(self.rotation);
        let move_amount = (amount * self.zoom_vis) * Camera::MOVE_SPEED_PER_S;
        let rotated_move = rot * Vector3::new(move_amount.x, move_amount.y, 1.0);
        self.set_position(self.position + rotated_move.xy());
    }

    pub fn rotate(&mut self, amount: f32) {
        self.set_rotation(self.rotation + amount * Camera::ROT_SPEED_PER_S);
    }

    pub fn zoom(&mut self, amount: f32) {
        self.set_zoom(self.zoom_raw + amount * Camera::ZOOM_SPEED_PER_S);
    }

    pub fn set_position(&mut self, new_position: Vector2<f32>) {
        self.position = new_position;
    }

    pub fn set_rotation(&mut self, new_rotation: f32) {
        self.rotation = new_rotation;
    }

    pub fn set_zoom(&mut self, new_zoom: f32) {
        // Zoom scales exponentially
        self.zoom_raw = new_zoom.clamp(-15.0, 15.0);
        self.zoom_vis = 1.5f32.powf(self.zoom_raw);
    }

    pub fn toggle_perspective_correction(&mut self) {
        self.perspective_correction = !self.perspective_correction;
    }

    pub fn matrix(&self) -> Matrix3<f32> {
        let rot = Matrix3::new_rotation(self.rotation);
        let zoom = Matrix3::new_scaling(self.zoom_vis);
        let trans = Matrix3::new_translation(&self.position);

        trans.mul(zoom).mul(rot)
    }

    pub fn perspective_correction(&self) -> bool {
        self.perspective_correction
    }
}
