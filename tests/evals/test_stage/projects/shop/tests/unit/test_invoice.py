import unittest

from shop.domain import Line, Order
from shop.invoice import invoice_lines


class InvoiceLinesTest(unittest.TestCase):
    def test_lists_each_line_and_the_total(self):
        order = Order(customer="ada", lines=[Line("desk", 99975, 1), Line("lamp", 11675, 2)], id=7)
        self.assertEqual(
            invoice_lines(order),
            [
                "Invoice for order 7 (ada)",
                "1 x desk: $999.75",
                "2 x lamp: $233.50",
                "Total: $1233.25",
            ],
        )
