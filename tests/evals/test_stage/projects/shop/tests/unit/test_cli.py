import unittest

from shop.cli import summary_line


class SummaryLineTest(unittest.TestCase):
    def test_one_line(self):
        order = {"id": 3, "lines": [{"sku": "pen"}], "total": "$1.25", "status": "placed"}
        self.assertEqual(summary_line(order), "Order 3: 1 line, $1.25, placed")

    def test_several_lines(self):
        order = {"id": 4, "lines": [{"sku": "pen"}, {"sku": "ink"}], "total": "$3.40", "status": "paid"}
        self.assertEqual(summary_line(order), "Order 4: 2 lines, $3.40, paid")
