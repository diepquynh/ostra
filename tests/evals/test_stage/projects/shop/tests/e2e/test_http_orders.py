import unittest

from tests.e2e.support import call, running_server

ORDER = {"customer": "ada", "lines": [{"sku": "pen", "unit_cents": 1234, "quantity": 2}]}


class OrdersOverHttpTest(unittest.TestCase):
    def test_place_then_show(self):
        with running_server() as url:
            status, created = call(url, "POST", "/orders", ORDER)
            self.assertEqual(status, 201)
            self.assertEqual(created["status"], "placed")
            self.assertEqual(created["total"], "$24.68")
            status, shown = call(url, "GET", f"/orders/{created['id']}")
            self.assertEqual(status, 200)
            self.assertEqual(shown, created)

    def test_pay(self):
        with running_server() as url:
            _, created = call(url, "POST", "/orders", ORDER)
            status, paid = call(url, "POST", f"/orders/{created['id']}/pay")
            self.assertEqual(status, 200)
            self.assertEqual(paid["status"], "paid")

    def test_an_unknown_order_is_404(self):
        with running_server() as url:
            status, body = call(url, "GET", "/orders/404")
            self.assertEqual(status, 404)
            self.assertIn("error", body)
