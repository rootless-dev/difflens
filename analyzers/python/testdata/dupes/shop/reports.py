"""Sales reports. Added by the branch."""

from __future__ import annotations

import csv
import json
import logging
import os
from collections import OrderedDict, defaultdict
from datetime import date, datetime, timedelta
from decimal import ROUND_HALF_UP, Decimal
from typing import (
    Any,
    Dict,
    Iterable,
    List,
    Optional,
)

log = logging.getLogger(__name__)


def daily_report(orders):
    """Paid orders of the day, largest first."""
    rows = []
    for order in orders:
        if order.state != "paid":
            continue
        net = sum(line.quantity * line.price for line in order.lines)
        tax = round(net * 0.2, 2)
        rows.append({"id": order.id, "net": net, "tax": tax, "gross": net + tax})
    rows.sort(key=lambda row: row["gross"], reverse=True)
    return rows


def weekly_report(orders, week):
    log.info("weekly report %s", week)
    rows = []
    for order in orders:
        if order.state != "paid":
            continue
        net = sum(line.quantity * line.price for line in order.lines)
        tax = round(net * 0.2, 2)
        rows.append({"id": order.id, "net": net, "tax": tax, "gross": net + tax})
    rows.sort(key=lambda row: row["gross"], reverse=True)
    print(len(rows), "orders in week", week)
    return rows


def monthly_report(orders, month):
    month = month or 1
    rows = []
    for order in orders:
        if order.state != "paid":
            continue
        net = sum(line.quantity * line.price for line in order.lines)
        tax = round(net * 0.2, 2)
        rows.append({"id": order.id, "net": net, "tax": tax, "gross": net + tax})
    rows.sort(key=lambda row: row["gross"], reverse=True)
    yield from rows
