import importlib.util
import json
from pathlib import Path
import struct
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("hpos_build", ROOT / "tools/build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class DiskImageTests(unittest.TestCase):
    def test_signature_partition_and_separate_disk_slots(self):
        image = build.make_image(b"boot", b"stage", b"kernel")
        self.assertEqual(image[:4], b"boot")
        self.assertEqual(image[510:512], b"\x55\xaa")
        part = struct.unpack_from("<B3sB3sII", image, 446)
        self.assertEqual((part[0], part[2], part[4]), (0x80, 0xda, 2048))
        self.assertEqual(part[4] + part[5], build.DATA_LBA)
        data = struct.unpack_from("<B3sB3sII", image, 462)
        self.assertEqual((data[0], data[2], data[4]), (0, 0x7f, build.DATA_LBA))
        self.assertEqual(data[4] + data[5], len(image) // 512)
        self.assertGreaterEqual(build.DATA_LBA * 512, 2048 * 512 + build.MAX_KERNEL_BYTES)
        self.assertEqual(image[512:517], b"stage")
        self.assertEqual(image[2048 * 512:2048 * 512 + 6], b"kernel")

    def test_volume_is_placed_in_the_data_partition(self):
        image = build.make_image(b"b", b"s", b"k", b"HPFS0001")
        self.assertEqual(image[build.DATA_LBA * 512:build.DATA_LBA * 512 + 8], b"HPFS0001")
        with self.assertRaises(ValueError):
            build.make_image(b"b", b"s", b"k", bytes(build.DATA_SECTORS * 512 + 1))

    def test_built_image_carries_a_mountable_hpfs_superblock(self):
        path = ROOT / "build/hpos.img"
        if not path.exists():
            self.skipTest("build the image first")
        with path.open("rb") as f:
            f.seek(build.DATA_LBA * 512)
            sector = f.read(512)
        self.assertEqual(sector[:8], b"HPFS0001")
        import zlib
        self.assertEqual(zlib.crc32(sector[:508]), struct.unpack_from("<I", sector, 508)[0])
        self.assertEqual(struct.unpack_from("<I", sector, 16)[0], build.DATA_SECTORS // 8)

    def test_rejects_data_that_overwrites_boot_regions(self):
        for mbr, stage, kernel in [
            (bytes(447), b"s", b"k"),
            (b"b", bytes(32 * 512 + 1), b"k"),
            (b"b", b"s", bytes(build.MAX_KERNEL_BYTES + 1)),
            (b"b", b"s", b""),
        ]:
            with self.assertRaises(ValueError):
                build.make_image(mbr, stage, kernel)


class StackFrameTests(unittest.TestCase):
    def test_probe_and_plain_frames_are_found(self):
        code = b"\x90" + b"\x49\x81\xeb" + (0x4f000).to_bytes(4, "little") + b"\x48\x81\xec" + (0x378).to_bytes(4, "little")
        self.assertEqual(build.largest_stack_frame(code), 0x4f000)
        self.assertEqual(build.largest_stack_frame(b"\x90\x90"), 0)

    def test_built_services_fit_their_stacks(self):
        manifest = ROOT / "build/manifest.json"
        if not manifest.exists():
            self.skipTest("build the image first")
        for name, info in json.loads(manifest.read_text())["services"].items():
            self.assertLessEqual(info["largest_stack_frame"], build.USER_STACK_BYTES // 2, name)


class ElfTests(unittest.TestCase):
    def test_built_kernel_includes_zero_initialized_storage(self):
        elf = ROOT / "target/x86_64-unknown-none/release/hpos-kernel"
        if not elf.exists():
            self.skipTest("build the kernel first")
        packed, base, entry = build.elf_image(elf)
        self.assertEqual(base, 0x100000)
        self.assertTrue(base <= entry < base + len(packed))
        self.assertEqual(packed, (ROOT / "build/kernel.bin").read_bytes())
        raw = elf.read_bytes()
        header = struct.unpack_from("<HHIQQQIHHHHHH", raw, 16)
        found_bss = False
        for i in range(header[9]):
            typ, _, offset, _, address, filesz, memsz, _ = struct.unpack_from(
                "<IIQQQQQQ", raw, header[4] + i * header[8])
            if typ == 1 and memsz > filesz:
                found_bss = True
                self.assertEqual(packed[address - base + filesz:address - base + memsz],
                                 bytes(memsz - filesz))
        self.assertTrue(found_bss, "the kernel's IDT must have zero-initialized storage")

    def test_actual_bootloader_entry_addresses(self):
        for name, address, budget in [("mbr", 0x7c00, 446), ("stage2", 0x8000, 16384)]:
            elf = ROOT / "build" / f"{name}.elf"
            if not elf.exists():
                self.skipTest("build the bootloader first")
            binary, base, entry = build.elf_image(elf, 32)
            self.assertEqual((base, entry), (address, address))
            self.assertLessEqual(len(binary), budget)

    def test_kernel_load_segments_separate_code_and_writable_storage(self):
        elf = ROOT / "target/x86_64-unknown-none/release/hpos-kernel"
        if not elf.exists():
            self.skipTest("build the kernel first")
        raw = elf.read_bytes()
        header = struct.unpack_from("<HHIQQQIHHHHHH", raw, 16)
        flags_seen = set()
        for i in range(header[9]):
            typ, flags, _, _, address, _, memsz, _ = struct.unpack_from(
                "<IIQQQQQQ", raw, header[4] + i * header[8])
            if typ == 1 and memsz:
                flags_seen.add(flags)
                self.assertNotEqual(flags & 3, 3, "writable code segment")
                self.assertEqual(address % 4096, 0)
        self.assertEqual(flags_seen, {4, 5, 6})

    def test_user_image_and_fault_resume_fit_the_copied_code_page(self):
        elf = ROOT / "target/x86_64-unknown-none/release/hpos-kernel"
        if not elf.exists():
            self.skipTest("build the kernel first")
        raw = elf.read_bytes()
        h = struct.unpack_from("<HHIQQQIHHHHHH", raw, 16)
        sections = [struct.unpack_from("<IIQQQQIIQQ", raw, h[5] + i * h[10]) for i in range(h[11])]
        symbols = {}
        for section in sections:
            if section[1] != 2:
                continue
            strings = sections[section[6]]
            names = raw[strings[4]:strings[4] + strings[5]]
            for offset in range(section[4], section[4] + section[5], section[9]):
                name, _, _, _, value, _ = struct.unpack_from("<IBBHQQ", raw, offset)
                end = names.find(b"\0", name)
                label = names[name:end].decode()
                if label.startswith("user_"):
                    symbols[label] = value
        start, end = symbols['user_program_start'], symbols['user_program_end']
        self.assertTrue(0 < end - start <= 4096)
        self.assertTrue(start < symbols['user_kernel_probe'] < symbols['user_probe_resume'] < end)
        for i in range(h[9]):
            typ, flags, _, _, address, _, memsz, _ = struct.unpack_from("<IIQQQQQQ", raw, h[4] + i * h[8])
            if typ == 1 and address <= start and end <= address + memsz:
                self.assertEqual(flags, 4, "user image source should be read-only data in the kernel")
                break
        else:
            self.fail('user image is not contained in a load segment')

    def test_bios_thunk_layout_matches_the_kernel_constants(self):
        elf = ROOT / "build/thunk.elf"
        if not elf.exists():
            self.skipTest("build the thunk first")
        binary, base, entry = build.elf_image(elf, 32)
        self.assertEqual((base, entry), (0x8000, 0x8000))
        self.assertTrue(0x1000 < len(binary) <= 0x2000)
        self.assertEqual(binary, (ROOT / "build/thunk.bin").read_bytes())
        # Request header at 0x9000: DAP size byte 16 and the 0x1000 bounce segment.
        self.assertEqual(binary[0x1008], 16)
        self.assertEqual(struct.unpack_from("<H", binary, 0x100e)[0], 0x1000)

    def test_services_have_separate_permissions_entry_and_valid_probe(self):
        for name in build.SERVICES:
            elf = ROOT / f'target/x86_64-unknown-none/release/hpos-{name}'
            if not elf.exists():
                self.skipTest('build the services first')
            packed, base, entry = build.elf_image(elf)
            symbols = build.elf_symbols(elf)
            self.assertEqual(base, 0xffffffff80000000)
            self.assertEqual(entry, base)
            self.assertEqual(packed, (ROOT / f'build/{name}.bin').read_bytes())
            self.assertLessEqual(len(packed), 0x80000)
            text, ro = symbols['__text_end'], symbols['__rodata_end']
            self.assertTrue(base < symbols['user_kernel_probe'] < symbols['user_probe_resume'] < text <= ro)
            raw = elf.read_bytes()
            h = struct.unpack_from('<HHIQQQIHHHHHH', raw, 16)
            for i in range(h[9]):
                typ, flags, _, _, address, _, memsz, _ = struct.unpack_from('<IIQQQQQQ', raw, h[4]+i*h[8])
                if typ != 1 or not memsz:
                    continue
                self.assertEqual(address % 4096, 0)
                self.assertNotEqual(flags & 3, 3)
                boundary = text if flags == 5 else ro if flags == 4 else base + len(packed)
                self.assertLessEqual(address + memsz, boundary)


if __name__ == "__main__":
    unittest.main()
