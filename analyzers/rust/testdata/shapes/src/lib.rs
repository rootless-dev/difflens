//! Shapes: a library with traits, generics, closures, threads and async.

pub mod generated;
pub mod geometry;

pub use geometry::total_area;

/// Anything with an area.
pub trait Shape {
    fn area(&self) -> f64;

    fn describe(&self) -> String {
        format!("a shape of area {:.2}", self.area())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Circle {
    pub radius: f64,
}

impl Circle {
    pub fn new(radius: f64) -> Self {
        Circle { radius }
    }

    pub fn unit() -> Self {
        Self::new(1.0)
    }
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
}

pub struct Square(pub f64);

impl Shape for Square {
    fn area(&self) -> f64 {
        self.0 * self.0
    }

    fn describe(&self) -> String {
        format!("a square of side {}", self.0)
    }
}
