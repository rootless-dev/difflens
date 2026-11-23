import typer

cli = typer.Typer()


@cli.command()
def import_orders(path: str) -> None:
    print(path)


@cli.command("export")
def export_orders() -> None:
    print("export")
