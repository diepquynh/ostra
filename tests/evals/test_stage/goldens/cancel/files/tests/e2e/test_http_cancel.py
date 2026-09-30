import unittest

from tests.e2e.support import call, running_server

ORDER = {"customer": "ada", "lines": [{"sku": "pen", "unit_cents": 1234, "quantity": 2}]}


def place(url):
    status, order = call(url, "POST", "/orders", ORDER)
    assert status == 201, order
    return order["id"]


class CancelOverHttpTest(unittest.TestCase):
    def test_cancel_a_placed_order_stores_the_status(self):
        with running_server() as url:
            order_id = place(url)
            status, cancelled = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 200)
            self.assertEqual(cancelled["status"], "cancelled")
            _, shown = call(url, "GET", f"/orders/{order_id}")
            self.assertEqual(shown["status"], "cancelled")

    def test_cancel_a_paid_order(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/pay")
            status, cancelled = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 200)
            self.assertEqual(cancelled["status"], "cancelled")

    def test_cancel_a_shipped_order_is_409(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/pay")
            call(url, "POST", f"/orders/{order_id}/ship")
            status, body = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 409)
            self.assertIn("shipped", body["error"])
            _, shown = call(url, "GET", f"/orders/{order_id}")
            self.assertEqual(shown["status"], "shipped")

    def test_cancel_twice_is_409(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/cancel")
            status, body = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 409)
            self.assertIn("already cancelled", body["error"])

    def test_cancel_an_unknown_order_is_404(self):
        with running_server() as url:
            status, _ = call(url, "POST", "/orders/999/cancel")
            self.assertEqual(status, 404)
