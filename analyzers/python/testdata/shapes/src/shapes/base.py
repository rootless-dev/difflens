import abc
import math
from typing import Protocol


class Shape(abc.ABC):
    def describe(self) -> str:
        return f"{self.name()} of area {self.area():.2f}"

    @abc.abstractmethod
    def area(self) -> float:
        ...

    def name(self) -> str:
        return type(self).__name__


class Square(Shape):
    def __init__(self, side: float) -> None:
        self.side = side

    def area(self) -> float:
        return self.side * self.side

    @staticmethod
    def unit() -> "Square":
        return Square(1)

    @classmethod
    def of(cls, side: float) -> "Square":
        return cls(side)


class Renderer(Protocol):
    def render(self, shape: Shape) -> str:
        ...


def area_of(shape: Shape) -> float:
    if shape is None:
        raise ValueError("no shape")
    value = shape.area()
    return round(value, 2) if value > 0 else math.nan
