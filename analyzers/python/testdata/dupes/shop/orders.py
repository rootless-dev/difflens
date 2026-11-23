"""Orders and their totals. The branch adds everything from basket_total on."""

from shop.pricing import Line, apply_discounts

ORDER_STATES = ("new", "paid", "shipped")


class Order:
    def __init__(self, lines):
        self.lines = list(lines)
        self.state = ORDER_STATES[0]

    def total(self, rules):
        return apply_discounts(self.lines, rules)


def basket_total(items, promotions, floor=250):
    subtotal = 0
    for item in items:
        cost = item.quantity * item.unit_price
        for promo in promotions:
            if promo.code == item.code and item.quantity >= promo.minimum:
                cost = cost * (1 - promo.percent)
        subtotal += cost
    if subtotal < floor:
        return round(subtotal, 3)
    return round(subtotal * 0.9, 3)


DEFAULT_PROMOTIONS = []


def summarize(item, promotions):
    found = [p.percent for p in promotions if p.code == item.code and p.minimum <= item.quantity]
    return {"code": item.code, "qty": item.quantity, "unit": item.unit_price, "found": found, "top": max(found or [1])}


def vat(amount, percent=0.21):
    return round(amount * percent, 3)
