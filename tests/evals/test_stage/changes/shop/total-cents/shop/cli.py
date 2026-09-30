"""A command-line client for the order API: `python3 -m shop.cli http://127.0.0.1:8080 3`."""

import json
import sys
import urllib.request

from .money import format_money


def fetch_order(base_url: str, order_id: int) -> dict:
    with urllib.request.urlopen(f"{base_url}/orders/{order_id}") as response:
        return json.load(response)


def summary_line(order: dict) -> str:
    count = len(order["lines"])
    noun = "line" if count == 1 else "lines"
    total = format_money(order["total_cents"])
    return f"Order {order['id']}: {count} {noun}, {total}, {order['status']}"


def main(argv=None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2:
        print("usage: python3 -m shop.cli BASE_URL ORDER_ID", file=sys.stderr)
        return 2
    print(summary_line(fetch_order(args[0], int(args[1]))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
