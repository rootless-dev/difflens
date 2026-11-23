import math

from shapes.base import Shape


class Circle(Shape):
    def __init__(self, radius: float) -> None:
        self.radius = radius

    def area(self) -> float:
        return math.pi * self.radius**2

    def name(self) -> str:
        return "circle"


class TextRenderer:
    def render(self, shape: Shape) -> str:
        return shape.describe()
