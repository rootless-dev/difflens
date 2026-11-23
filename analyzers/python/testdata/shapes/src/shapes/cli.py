import argparse
import asyncio
import signal
import sys

import click

from shapes.service import draw_all


@click.group()
def cli() -> None:
    """Shape tools."""


@cli.command()
@click.argument("names", nargs=-1)
def draw_command(names: tuple[str, ...]) -> None:
    for line in asyncio.run(draw_all(list(names))):
        click.echo(line)


def on_term(signum, frame) -> None:
    sys.exit(0)


def list_shapes(args: argparse.Namespace) -> int:
    print("circle square")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="shapes")
    commands = parser.add_subparsers()
    listing = commands.add_parser("list")
    listing.set_defaults(func=list_shapes)
    signal.signal(signal.SIGTERM, on_term)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
