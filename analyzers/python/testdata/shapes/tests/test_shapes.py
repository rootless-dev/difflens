import unittest

import pytest

from shapes import Circle, area_of
from shapes.base import Square


@pytest.fixture
def circle():
    return Circle(1)


def test_circle_area(circle):
    assert area_of(circle) == 3.14


class TestSquare:
    def test_unit(self):
        assert Square.unit().area() == 1


class SquareCase(unittest.TestCase):
    def setUp(self):
        self.square = Square(2)

    def test_describe(self):
        self.assertIn("area 4.00", self.square.describe())
