"""Orders and the statuses they move through."""

from dataclasses import dataclass

from .pricing import line_total

PLACED = "placed"
PAID = "paid"
SHIPPED = "shipped"


class OrderError(Exception):
    """A request the order rules refuse."""


class NotFound(OrderError):
    """No order has the given id."""


class InvalidState(OrderError):
    """The order's status does not allow the change."""


@dataclass
class Line:
    sku: str
    unit_cents: int
    quantity: int


@dataclass
class Order:
    customer: str
    lines: list[Line]
    status: str = PLACED
    id: int | None = None

    def total_cents(self) -> int:
        return sum(line_total(line.unit_cents, line.quantity) for line in self.lines)
