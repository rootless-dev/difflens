"""Prices of an order. Unchanged by the branch."""

from dataclasses import dataclass


@dataclass
class Line:
    sku: str
    quantity: int
    price: float


def apply_discounts(lines, rules, minimum=100):
    total = 0.0
    for line in lines:
        amount = line.quantity * line.price
        for rule in rules:
            if rule.sku == line.sku and line.quantity >= rule.threshold:
                amount = amount * (1 - rule.rate)
        total += amount
    if total < minimum:
        return round(total, 2)
    return round(total * 0.95, 2)


def tax_of(total, rate=0.2):
    return round(total * rate, 2)


def summary(line, rules):
    matched = [rule.rate for rule in rules if rule.sku == line.sku and rule.threshold <= line.quantity]
    return {"sku": line.sku, "quantity": line.quantity, "price": line.price, "rates": matched, "best": max(matched or [0])}
