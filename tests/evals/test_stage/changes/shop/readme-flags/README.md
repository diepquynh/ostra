# shop

A small order service: a JSON HTTP API over SQLite, a command-line client for it, and invoice text. Python 3.12,
standard library only.

- `shop/money.py`: amounts are integer cents; `to_cents` parses text and `format_money` prints it.
- `shop/pricing.py`: line prices.
- `shop/domain.py`: orders, lines, and the statuses an order moves through (placed, paid, shipped).
- `shop/repository.py`: orders stored in SQLite.
- `shop/service.py`: the order rules.
- `shop/api.py`: the routes and the JSON an order becomes.
- `shop/server.py`: the HTTP server around the API: `python3 -m shop.server --db shop.db --port 8080`.
  `--db` is the SQLite file (created when missing) and `--port` the port on 127.0.0.1.
- `shop/cli.py`: a client that prints one line per order: `python3 -m shop.cli http://127.0.0.1:8080 3`.
- `shop/invoice.py`: invoice text for an order.

The storefront project (`../storefront`) renders the orders it reads from `GET /orders/<id>`.

Tests live under `tests/`, one folder per level: `unit`, `integration`, and `e2e`. `.ostra/project.toml` has
the command for each.
