"""HTTP routes: requests in, JSON out. The server passes every request to `Api.handle`."""

import json
import re

from .domain import InvalidState, Line, NotFound
from .money import format_money

ROUTES = [
    ("POST", re.compile(r"^/orders$"), "create"),
    ("GET", re.compile(r"^/orders/(\d+)$"), "show"),
    ("POST", re.compile(r"^/orders/(\d+)/pay$"), "pay"),
    ("POST", re.compile(r"^/orders/(\d+)/ship$"), "ship"),
    ("POST", re.compile(r"^/orders/(\d+)/cancel$"), "cancel"),
]


def order_json(order) -> dict:
    return {
        "id": order.id,
        "customer": order.customer,
        "status": order.status,
        "lines": [
            {"sku": line.sku, "unit_cents": line.unit_cents, "quantity": line.quantity}
            for line in order.lines
        ],
        "total": format_money(order.total_cents()),
    }


class Api:
    def __init__(self, service):
        self.service = service

    def handle(self, method: str, path: str, body: bytes = b"") -> tuple[int, dict]:
        for verb, pattern, name in ROUTES:
            match = pattern.match(path)
            if match is None or verb != method:
                continue
            try:
                return getattr(self, name)(*match.groups(), body=body)
            except NotFound as error:
                return 404, {"error": str(error)}
            except InvalidState as error:
                return 409, {"error": str(error)}
            except ValueError as error:
                return 400, {"error": str(error)}
        return 404, {"error": f"no route for {method} {path}"}

    def create(self, body: bytes) -> tuple[int, dict]:
        try:
            data = json.loads(body or b"{}")
            customer = str(data["customer"])
            lines = [
                Line(str(line["sku"]), int(line["unit_cents"]), int(line["quantity"]))
                for line in data["lines"]
            ]
        except (KeyError, TypeError) as error:
            raise ValueError(f"not an order: missing {error}") from error
        return 201, order_json(self.service.place(customer, lines))

    def show(self, order_id: str, body: bytes) -> tuple[int, dict]:
        return 200, order_json(self.service.get(int(order_id)))

    def pay(self, order_id: str, body: bytes) -> tuple[int, dict]:
        return 200, order_json(self.service.pay(int(order_id)))

    def ship(self, order_id: str, body: bytes) -> tuple[int, dict]:
        return 200, order_json(self.service.ship(int(order_id)))

    def cancel(self, order_id: str, body: bytes) -> tuple[int, dict]:
        return 200, order_json(self.service.cancel(int(order_id)))
