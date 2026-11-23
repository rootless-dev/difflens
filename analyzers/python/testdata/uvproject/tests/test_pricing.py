import pytest

import orders
from orders.pricing import shipping, total


def test_version():
    assert orders.__version__ == "0.3.0"


@pytest.mark.parametrize("prices,expected", [([1.0, 2.0], 3.0), ([], 0.0)])
def test_total(prices, expected):
    assert total(prices) == expected


class TestShipping:
    def test_light(self):
        assert shipping(0.5) == 5.0

    @pytest.mark.skip(reason="not yet")
    def test_heavy(self):
        assert shipping(10) == 25.0
