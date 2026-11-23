_ORDERS: dict[int, dict] = {}


def find(order_id: int) -> dict:
    return _ORDERS.get(order_id, {})


def save(payload: dict) -> dict:
    order = {"id": len(_ORDERS) + 1, **payload}
    _ORDERS[order["id"]] = order
    return order
