import unittest

import pytest

from calc import add, divide


def test_add():
    assert add(1, 2) == 3


def test_add_wrong():
    print("about to fail")
    assert add(1, 2) == 4


@pytest.mark.parametrize("a,b", [(1, 1), (4, 2), ("a/b", 1)])
def test_divide(a, b):
    assert divide(a, b) >= 1


class DivideCase(unittest.TestCase):
    def test_by_zero(self):
        with self.assertRaises(ZeroDivisionError):
            divide(1, 0)
