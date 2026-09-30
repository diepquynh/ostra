"""Line prices."""


def line_total(unit_cents: int, quantity: int) -> int:
    """The price of `quantity` units at `unit_cents` each."""
    if quantity <= 0:
        raise ValueError("quantity must be positive")
    return unit_cents * quantity
