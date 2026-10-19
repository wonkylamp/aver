import pytest

from greet import greet


@pytest.mark.parametrize("name", ["aver", "uv"])
def test_greet_says_hello(name):
    assert greet(name) == f"hello, {name}"
