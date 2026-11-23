"""A small account module: the functions the mutation tests mutate."""


def can_withdraw(balance, amount):
    """Whether amount can be taken from balance. The tests never try the boundary."""
    if balance >= amount:
        return True
    return False


def apply_fee(amount, rate):
    """amount with its fee added. Well tested: every mutant is killed."""
    fee = amount * rate
    return amount + fee


def double_steps(n):
    """Two steps per round, n rounds. `i -= 1` instead of `i += 1` never ends."""
    steps = 0
    i = 0
    while i < n:
        steps += 2
        i += 1
    return steps


def never_called(value):
    """Changed, but no test runs it: it is not mutated."""
    return value - 1
