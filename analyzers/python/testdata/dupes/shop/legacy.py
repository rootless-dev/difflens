"""What the old checkout kept. Unchanged by the branch."""

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


def export_orders(orders, path):
    with open(path, "w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["id", "state", "total"])
        for order in orders:
            if order.state == "cancelled":
                continue
            writer.writerow([order.id, order.state, order.total])
    log.info("exported %d orders to %s", len(orders), path)
    return path


def export_refunds(refunds, path):
    with open(path, "w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["id", "state", "total"])
        for refund in refunds:
            if refund.state == "cancelled":
                continue
            writer.writerow([refund.id, refund.state, refund.total])
    log.info("exported %d refunds to %s", len(refunds), path)
    return path
