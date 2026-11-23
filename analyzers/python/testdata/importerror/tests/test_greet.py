from app.greet import greet


def test_greet():
    assert greet("ada") == "hello ada"
