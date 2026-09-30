"""Invoice text for an order."""

from .money import format_money
from .pricing import line_total


def invoice_lines(order) -> list[str]:
    rows = [f"Invoice for order {order.id} ({order.customer})"]
    for line in order.lines:
        amount = format_money(line_total(line.unit_cents, line.quantity))
        rows.append(f"{line.quantity} x {line.sku}: {amount}")
    rows.append(f"Total: {format_money(order.total_cents())}")
    return rows
