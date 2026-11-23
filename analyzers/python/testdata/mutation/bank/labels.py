"""A module the account tests do not import: its tests are not run for account mutants."""


def label(amount):
    return "big" if amount > 1000 else "small"
