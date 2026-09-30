"""Line prices."""

# (minimum quantity, percent off), largest first.
BULK_TIERS = ((50, 10), (10, 5))


def line_total(unit_cents: int, quantity: int) -> int:
    """The price of `quantity` units at `unit_cents` each."""
    if quantity <= 0:
        raise ValueError("quantity must be positive")
    return unit_cents * quantity


def bulk_discount(quantity: int, unit_cents: int) -> int:
    """Cents off a line of `quantity` units: 5% from 10 units, 10% from 50, rounded down to whole cents."""
    if quantity <= 0:
        raise ValueError("quantity must be positive")
    if unit_cents < 0:
        raise ValueError("the unit price must not be negative")
    for minimum, percent in BULK_TIERS:
        if quantity >= minimum:
            return unit_cents * quantity * percent // 100
    return 0
