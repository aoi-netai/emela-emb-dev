#!/usr/bin/env python3
"""Synthetic accounting regressions; no target execution or external packages."""
import unittest

from check_stm32f4_resources import FLASH_START, RAM_START, RAM_SIZE, Section, sections_from_objdump, symbols_from_nm, validate


def fixture():
    top = RAM_START + RAM_SIZE
    bottom = top - 16384
    guard = bottom - 256
    sections = [
        Section(".text", 512, FLASH_START, FLASH_START, ["CONTENTS", "ALLOC", "LOAD"]),
        Section(".data", 32, RAM_START, FLASH_START + 512, ["CONTENTS", "ALLOC", "LOAD"]),
        Section(".bss", 1024, RAM_START + 32, RAM_START + 32, ["ALLOC"]),
        Section(".emela_stack_guard", 256, guard, guard, ["ALLOC"]),
        Section(".emela_stack", 16384, bottom, bottom, ["ALLOC"]),
    ]
    symbols = {"__emela_stack_top__": top, "__emela_stack_bottom__": bottom,
               "__emela_stack_guard_start__": guard, "__emela_stack_guard_end__": bottom,
               "__emela_static_ram_end__": RAM_START + 1056, "_estack": top}
    frames = [{"function": "work", "bytes": 80, "kind": "static", "file": "work.su"}]
    observed = {"used": 512, "capacity": 16384, "painted": 16376, "guard_ok": 1,
                "samples": 2, "initial_sp": top - 8, "min_sp": top - 256}
    return sections, symbols, frames, observed


class ResourceTests(unittest.TestCase):
    def report(self, **options):
        sections, symbols, frames, observed = fixture()
        return validate(sections, symbols, frames, observed=observed, **options)

    def test_data_lma_counts_flash_once_and_noload_stack_does_not_fill_ram_gap(self):
        report = self.report()
        self.assertTrue(report["ok"], report["errors"])
        self.assertEqual(report["flash"]["loaded_union_bytes"], 544)
        self.assertEqual(report["flash"]["image_span_bytes"], 544)
        self.assertEqual(report["ram"]["allocated_union_bytes"], 1056 + 256 + 16384)
        self.assertEqual(report["ram"]["static_allocated_bytes"], 1056)
        self.assertGreater(report["ram"]["unallocated_gap_bytes"], 100000)
        self.assertFalse(report["stack"]["whole_call_stack_bound_proved"])

    def test_flash_ram_stack_and_individual_frame_budgets_fail(self):
        for options, word in [({"flash_budget": 543}, "Flash image"), ({"ram_budget": 1024}, "RAM allocated"),
                              ({"stack_budget": 4096}, "MSP reservation"), ({"max_frame_bytes": 79}, "individual GCC frame")]:
            report = self.report(**options)
            self.assertFalse(report["ok"])
            self.assertTrue(any(word in error for error in report["errors"]))

    def test_guard_corruption_and_invalid_observed_stack_fail(self):
        for key, value in [("guard_ok", 0), ("used", 16385), ("samples", 1), ("min_sp", RAM_START), ("painted", 100)]:
            sections, symbols, frames, observed = fixture()
            observed[key] = value
            report = validate(sections, symbols, frames, observed=observed)
            self.assertFalse(report["ok"], (key, report))

    def test_msp_layout_and_ram_section_overlap_fail(self):
        sections, symbols, frames, observed = fixture()
        symbols["_estack"] -= 256
        self.assertFalse(validate(sections, symbols, frames)["ok"])
        sections[2].size = RAM_SIZE
        report = validate(sections, fixture()[1], frames)
        self.assertTrue(any("overlap" in error for error in report["errors"]))

    def test_flash_lma_overlap_and_physical_overflow_fail(self):
        sections, symbols, frames, observed = fixture()
        sections[1].lma = FLASH_START + 256
        self.assertTrue(any("ranges overlap" in error for error in validate(sections, symbols, frames)["errors"]))
        sections[1].lma = FLASH_START + 512 * 1024 - 16
        self.assertFalse(validate(sections, symbols, frames)["ok"])

    def test_linked_allocators_fail_but_fixed_heap_symbols_are_allowed(self):
        for name in ["malloc", "free", "_calloc_r", "_realloc_r", "_sbrk", "__malloc_lock"]:
            sections, symbols, frames, observed = fixture()
            symbols[name] = FLASH_START
            self.assertIn(name, validate(sections, symbols, frames)["linked_dynamic_allocators"])
        sections, symbols, frames, observed = fixture()
        symbols["emela_heap_free_block"] = FLASH_START
        self.assertTrue(validate(sections, symbols, frames)["ok"])

    def test_missing_and_unbounded_stack_records_fail(self):
        sections, symbols, frames, observed = fixture()
        self.assertFalse(validate(sections, symbols, [])["ok"])
        frames[0]["kind"] = "dynamic"
        self.assertFalse(validate(sections, symbols, frames)["ok"])
        frames[0]["kind"] = "dynamic,bounded"
        self.assertTrue(validate(sections, symbols, frames)["ok"])

    def test_objdump_and_nm_parsers_accept_real_layout_forms(self):
        sections = sections_from_objdump("""  0 .text 00000200 08000000 08000000 00001000 2**2
                  CONTENTS, ALLOC, LOAD, READONLY, CODE
  1 .emela_stack 00004000 2001c000 2001c000 00002000 2**0 ALLOC
""")
        self.assertEqual(sections[0].size, 512)
        self.assertEqual(sections[1].flags, ["ALLOC"])
        self.assertEqual(symbols_from_nm("20020000 A _estack\n08000000 00000020 T work\n"), {"_estack": 0x20020000, "work": FLASH_START})


if __name__ == "__main__":
    unittest.main()
