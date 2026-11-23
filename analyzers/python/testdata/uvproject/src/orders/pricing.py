def total(prices: list[float], discount: float = 0.0) -> float:
    subtotal = sum(prices)
    if discount:
        subtotal -= subtotal * discount
    return round(subtotal, 2)


def shipping(weight: float) -> float:
    if weight <= 0:
        raise ValueError("weight must be positive")
    if weight < 1:
        return 5.0
    return 5.0 + weight * 2


def never_called(value: int) -> int:
    doubled = value * 2
    return doubled + 1
