//! Sums, descriptions and classifications of shapes.

use std::thread;

use crate::{Circle, Shape};

/// The total area of some circles.
pub fn total_area(circles: &[Circle]) -> f64 {
    circles.iter().map(|circle| scaled(circle.area(), 1.0)).sum()
}

pub fn scaled(value: f64, factor: f64) -> f64 {
    fn helper(value: f64) -> f64 {
        value
    }
    helper(value) * factor
}

/// The descriptions of the shapes larger than the first.
pub fn describe_all<S: Shape>(first: &S, others: &[S]) -> Vec<String> {
    let mut out = vec![first.describe()];
    for shape in others {
        if shape.area() > first.area() {
            out.push(shape.describe());
        }
    }
    out
}

pub fn print_boxed(shape: Box<dyn Shape>) -> String {
    shape.describe()
}

pub fn unit_circle_area() -> f64 {
    let circle = Circle::new(1.0);
    circle.area()
}

pub(crate) fn parse_size(text: &str) -> Result<f64, std::num::ParseFloatError> {
    text.trim().parse::<f64>()
}

pub fn classify(text: &str) -> Result<&'static str, std::num::ParseFloatError> {
    let size = parse_size(text)?;
    let size = scaled(size, 2.0);
    if size > 10.0 && size < 100.0 {
        Ok("large")
    } else if size > 1.0 {
        Ok("medium")
    } else {
        Ok("small")
    }
}

pub fn clamp_size(size: Option<f64>) -> f64 {
    match size {
        Some(side) if side > scaled(1.0, 2.0) => side,
        _ => 0.0,
    }
}

pub fn area_in_background(circle: Circle) -> f64 {
    let handle = thread::spawn(move || circle.area());
    handle.join().unwrap_or(0.0)
}

/// Stands in for an executor's spawn.
pub fn spawn<F: std::future::Future>(future: F) -> F {
    future
}

pub async fn refresh(circles: Vec<Circle>) -> f64 {
    let pending = spawn(async move { total_area(&circles) });
    pending.await
}

pub fn areas(values: &[f64]) -> Vec<f64> {
    let apply: fn(f64, f64) -> f64 = scaled;
    values.iter().map(|value| apply(*value, 2.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeShape;

    impl Shape for FakeShape {
        fn area(&self) -> f64 {
            0.0
        }
    }

    #[test]
    fn totals_two_circles() {
        let circles = [Circle::new(1.0), Circle::new(2.0)];
        assert!(total_area(&circles) > 15.0);
        assert_eq!(FakeShape.area(), 0.0);
    }

    #[test]
    fn classifies_sizes() {
        assert_eq!(classify("20"), Ok("large"));
        assert_eq!(classify("0.1"), Ok("small"));
    }
}
