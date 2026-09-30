import unittest

from tests.e2e.support import call, running_server

ORDER = {"customer": "ada", "lines": [{"sku": "pen", "unit_cents": 1234, "quantity": 2}]}


def place(url):
    status, order = call(url, "POST", "/orders", ORDER)
    assert status == 201, order
    return order["id"]


class OrderLifecycleOverHttpTest(unittest.TestCase):
    def test_place_pay_and_ship(self):
        with running_server() as url:
            order_id = place(url)
            status, paid = call(url, "POST", f"/orders/{order_id}/pay")
            self.assertEqual((status, paid["status"]), (200, "paid"))
            status, shipped = call(url, "POST", f"/orders/{order_id}/ship")
            self.assertEqual((status, shipped["status"]), (200, "shipped"))
            _, shown = call(url, "GET", f"/orders/{order_id}")
            self.assertEqual(shown["status"], "shipped")

    def test_cancel_a_placed_order(self):
        with running_server() as url:
            order_id = place(url)
            status, cancelled = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual((status, cancelled["status"]), (200, "cancelled"))
            _, shown = call(url, "GET", f"/orders/{order_id}")
            self.assertEqual(shown["status"], "cancelled")

    def test_cancel_a_paid_order(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/pay")
            status, cancelled = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual((status, cancelled["status"]), (200, "cancelled"))

    def test_ship_before_pay_is_409(self):
        with running_server() as url:
            order_id = place(url)
            status, body = call(url, "POST", f"/orders/{order_id}/ship")
            self.assertEqual(status, 409)
            self.assertIn("error", body)
            _, shown = call(url, "GET", f"/orders/{order_id}")
            self.assertEqual(shown["status"], "placed")

    def test_pay_twice_is_409(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/pay")
            status, _ = call(url, "POST", f"/orders/{order_id}/pay")
            self.assertEqual(status, 409)

    def test_cancel_a_shipped_order_is_409(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/pay")
            call(url, "POST", f"/orders/{order_id}/ship")
            status, _ = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 409)

    def test_cancel_twice_is_409(self):
        with running_server() as url:
            order_id = place(url)
            call(url, "POST", f"/orders/{order_id}/cancel")
            status, _ = call(url, "POST", f"/orders/{order_id}/cancel")
            self.assertEqual(status, 409)

    def test_an_order_without_lines_is_400(self):
        with running_server() as url:
            status, body = call(url, "POST", "/orders", {"customer": "ada", "lines": []})
            self.assertEqual(status, 400)
            self.assertIn("error", body)

    def test_an_order_without_a_customer_is_400(self):
        with running_server() as url:
            status, _ = call(url, "POST", "/orders", {"lines": ORDER["lines"]})
            self.assertEqual(status, 400)

    def test_paying_an_unknown_order_is_404(self):
        with running_server() as url:
            status, _ = call(url, "POST", "/orders/999/pay")
            self.assertEqual(status, 404)
