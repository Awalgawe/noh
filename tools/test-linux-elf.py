"""Linux delivery rejection checks with tiny synthetic ELF images."""
import struct
import unittest

import linux_elf


def fixture(needed=(), rpath="$ORIGIN/lib", soname=None, glibc=None,
            interpreter=linux_elf.INTERPRETER):
    data = bytearray(2048)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHI", data, 16, 3, 62, 1)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HHH", data, 52, 64, 56, 3 if interpreter else 2)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0x400000, 0, len(data), len(data), 4096)
    strings = bytearray(b"\0")

    def string(text):
        index = len(strings)
        strings.extend(text.encode() + b"\0")
        return index

    tags = [(1, string(name)) for name in needed]
    if rpath is not None:
        tags.append((15, string(rpath)))
    if soname:
        tags.append((14, string(soname)))
    if glibc:
        file_index = string("libc.so.6")
        version_index = string(glibc)
        tags += [(0x6ffffffe, 0x400500), (0x6fffffff, 1)]
        struct.pack_into("<HHIII", data, 0x500, 1, 1, file_index, 16, 0)
        struct.pack_into("<IHHII", data, 0x510, 0, 0, 2, version_index, 0)
    tags += [(5, 0x400300), (10, len(strings)), (0, 0)]
    assert len(tags) * 16 <= 0x100 and len(strings) < 0x200
    for index, pair in enumerate(tags):
        struct.pack_into("<qQ", data, 0x200 + index * 16, *pair)
    struct.pack_into("<IIQQQQQQ", data, 120, 2, 4, 0x200, 0x400200, 0,
                     len(tags) * 16, len(tags) * 16, 8)
    data[0x300:0x300 + len(strings)] = strings
    if interpreter:
        value = interpreter.encode() + b"\0"
        struct.pack_into("<IIQQQQQQ", data, 176, 3, 4, 0x600, 0x400600, 0, len(value), len(value), 1)
        data[0x600:0x600 + len(value)] = value
    return bytes(data)


class ElfDeliveryTests(unittest.TestCase):
    def test_relocatable_closure_and_glibc_requirements(self):
        report = linux_elf.audit_images([
            ("usr/lib/noh/noh-app", fixture(("libnative.so.1", "libc.so.6"), glibc="GLIBC_2.39")),
            ("usr/lib/noh/lib/libnative.so.1", fixture(("libm.so.6",), "$ORIGIN", "libnative.so.1", interpreter=None)),
        ])
        imports = report["images"]["usr/lib/noh/noh-app"]["dependencies"]
        self.assertEqual(imports["libnative.so.1"]["path"], "usr/lib/noh/lib/libnative.so.1")
        self.assertEqual(imports["libc.so.6"]["kind"], "host_system_or_graphics_driver")
        self.assertEqual(report["unresolved"], [])

    def test_missing_library_cannot_be_satisfied_from_system_path(self):
        with self.assertRaisesRegex(ValueError, "Unresolved"):
            linux_elf.audit_images([("app", fixture(("libnotbundled.so",)))])

    def test_each_library_must_resolve_its_own_transitive_imports(self):
        with self.assertRaisesRegex(ValueError, "Unresolved"):
            linux_elf.audit_images([
                ("app", fixture(("liba.so",))),
                ("lib/liba.so", fixture(("libb.so",), None)),
                ("lib/libb.so", fixture((), None)),
            ])

    def test_unrelated_same_named_library_is_not_a_provider(self):
        with self.assertRaisesRegex(ValueError, "Unresolved"):
            linux_elf.audit_images([("app", fixture(("liba.so",))), ("elsewhere/liba.so", fixture())])

    def test_no_absolute_cwd_or_external_search_paths(self):
        for value in ("/usr/lib", "", "$ORIGIN:", "$ORIGIN/../..", "$ORIGIN/$PLATFORM", "${ORIGIN}"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                linux_elf.audit_images([("bin/app", fixture(rpath=value))])

    def test_parent_search_inside_bundle_is_valid(self):
        report = linux_elf.audit_images([
            ("bin/app", fixture(("liba.so",), "$ORIGIN/../lib")),
            ("lib/liba.so", fixture((), "$ORIGIN", "liba.so", interpreter=None)),
        ])
        self.assertEqual(report["images"]["bin/app"]["dependencies"]["liba.so"]["path"], "lib/liba.so")

    def test_architecture_and_program_extent_are_checked(self):
        for offset, fmt, value in ((18, "<H", 183), (64 + 32, "<Q", 999999), (32, "<Q", 2040)):
            data = bytearray(fixture())
            struct.pack_into(fmt, data, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                linux_elf.elf_info(data)
        with self.assertRaises(ValueError):
            linux_elf.elf_info(fixture()[:24])

    def test_interpreter_is_an_explicit_host_contract(self):
        with self.assertRaisesRegex(ValueError, "interpreter"):
            linux_elf.audit_images([("app", fixture(interpreter="/tmp/loader"))])

    def test_cannot_bundle_libc_or_graphics_driver_libraries(self):
        for name in ("libc.so.6", "libGL.so.1", "ld-linux-x86-64.so.2"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "must not be bundled"):
                linux_elf.audit_images([(f"lib/{name}", fixture())])

    def test_soname_and_import_paths_are_checked(self):
        with self.assertRaisesRegex(ValueError, "SONAME"):
            linux_elf.audit_images([
                ("app", fixture(("liba.so",))),
                ("lib/liba.so", fixture(soname="libb.so")),
            ])
        with self.assertRaisesRegex(ValueError, "Unsafe ELF import"):
            linux_elf.audit_images([("app", fixture(("../libx.so",)))])

    def test_glibc_future_and_private_versions_are_rejected(self):
        for version in ("GLIBC_2.40", "GLIBC_PRIVATE", "GLIBC_FUTURE_ABI"):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "unsupported glibc"):
                linux_elf.audit_images([("app", fixture(("libc.so.6",), glibc=version))])
        linux_elf.audit_images([("app", fixture(("libc.so.6",), glibc="GLIBC_ABI_DT_RELR"))])

    def test_versions_must_name_an_imported_library(self):
        with self.assertRaisesRegex(ValueError, "matching import"):
            linux_elf.elf_info(fixture(glibc="GLIBC_2.39"))

    def test_version_chain_cannot_escape_or_lie_about_count(self):
        for offset, value in ((0x500 + 8, 999999), (0x500 + 12, 16), (0x510 + 12, 16)):
            data = bytearray(fixture(("libc.so.6",), glibc="GLIBC_2.39"))
            struct.pack_into("<I", data, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                linux_elf.elf_info(data)

    def test_dynamic_table_requires_terminator_and_unique_singletons(self):
        data = bytearray(fixture())
        struct.pack_into("<qQ", data, 0x230, 15, 0)
        with self.assertRaisesRegex(ValueError, "Unterminated"):
            linux_elf.elf_info(data)
        data = bytearray(fixture())
        struct.pack_into("<qQ", data, 0x200, 5, 0x400300)
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            linux_elf.elf_info(data)

    def test_dynamic_table_must_match_the_loaders_file_backed_address(self):
        # Decoy DT_NULL bytes must not hide the table used by the OS loader.
        for offset, value in ((120 + 8, 0x700), (120 + 16, 0x400700),
                              (120 + 16, 0x500000), (120 + 40, 0)):
            data = bytearray(fixture(("libmissing.so",)))
            struct.pack_into("<Q", data, offset, value)
            with self.subTest(offset=offset, value=value), self.assertRaises(ValueError):
                linux_elf.audit_images([("app", bytes(data))])

    def test_bundle_paths_and_duplicate_images_are_rejected(self):
        for name in ("../app", "/app", "./app", "a/../app", "a\\app", "C:app"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Unsafe Linux"):
                linux_elf.audit_images([(name, fixture())])
        with self.assertRaisesRegex(ValueError, "Duplicate Linux"):
            linux_elf.audit_images([("app", fixture()), ("app", fixture())])


if __name__ == "__main__":
    unittest.main()
