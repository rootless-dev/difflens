import os
import sys


def count(items: list[int]) -> int:
    return len(items)


def wrong() -> int:
    return "not a number"


def defaults(values=[]):
    values.append(1)
    return count(values)
