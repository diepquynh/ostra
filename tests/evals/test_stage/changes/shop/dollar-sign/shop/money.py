"""Money amounts. Every amount is an integer number of cents; text appears only at the edges."""


def to_cents(amount: str) -> int:
    """Parse a decimal amount such as "12.34", "$12.34", "12", or "-$0.50" into cents."""
    text = amount.strip()
    negative = text.startswith("-")
    if negative:
        text = text[1:]
    text = text.removeprefix("$")
    whole, _, frac = text.partition(".")
    if not whole.isdigit() or len(frac) > 2 or (frac and not frac.isdigit()):
        raise ValueError(f"not an amount: {amount!r}")
    cents = int(whole) * 100 + int(frac or "0")
    return -cents if negative else cents


def format_money(cents: int) -> str:
    """Format cents as dollars: 123456 becomes "$1234.56" and -5 becomes "-$0.05"."""
    sign = "-" if cents < 0 else ""
    whole, frac = divmod(abs(cents), 100)
    return f"{sign}${whole}.{frac:02d}"
