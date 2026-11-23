import asyncio
import functools
import threading
from concurrent.futures import ThreadPoolExecutor

from . import base
from .base import Renderer, Shape, Square
from .circle import Circle, TextRenderer


def retry(times: int):
    def decorate(func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            for _ in range(times):
                try:
                    return func(*args, **kwargs)
                except OSError:
                    continue
            return func(*args, **kwargs)

        return wrapper

    return decorate


class Drawing:
    def __init__(self, renderer: Renderer) -> None:
        self.renderer = renderer
        self.shapes: list[Shape] = []
        self.pool = ThreadPoolExecutor()

    def add(self, shape: Shape) -> "Drawing":
        self.shapes.append(shape)
        return self

    @retry(3)
    def render(self) -> list[str]:
        out = [self.renderer.render(s) for s in self.shapes if s]
        self.pool.submit(self.log, len(out))
        return out

    def log(self, count: int) -> None:
        print(f"{count} shapes")

    def total(self) -> float:
        return sum(base.area_of(s) for s in self.shapes)


async def fetch(name: str) -> Shape:
    await asyncio.sleep(0)
    return Circle(1) if name == "circle" else Square.of(2)


async def draw_all(names: list[str]) -> list[str]:
    drawing = Drawing(TextRenderer())
    tasks = [asyncio.create_task(fetch(n)) for n in names]
    background = asyncio.create_task(fetch("square"))
    for shape in await asyncio.gather(*tasks):
        drawing.add(shape)
    await background
    threading.Thread(target=drawing.render, daemon=True).start()
    unit = Square.unit()
    drawing.add(unit).add(Circle(2))
    key = sorted(names, key=lambda n: len(n))
    del key
    return drawing.render()
