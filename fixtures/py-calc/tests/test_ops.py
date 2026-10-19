import pytest

from calc import add, mul, sub


def test_add_returns_the_sum():
    assert add(2, 3) == 5


def test_sub_returns_the_difference():
    assert sub(5, 3) == 2


class TestMul:
    @pytest.mark.parametrize("a,b,product", [(4, 3, 12), (0, 7, 0)])
    def test_mul_returns_the_product(self, a, b, product):
        assert mul(a, b) == product
