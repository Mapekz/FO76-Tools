#!/usr/bin/env python3
"""Tests for tools/extractor/extract.py leaf parsers."""

import sys
import unittest
from pathlib import Path

# Add parent directory to path to import extract module
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from extractor.extract import Extractor


class TestParseInteger(unittest.TestCase):
    """Tests for _parse_integer method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_integer_u32_with_sig(self):
        """Test u32 integer with signature."""
        result = self.ext._parse_integer("wbInteger(EDID, 'Editor ID', itU32)")
        self.assertEqual(result, {
            'kind': 'integer',
            'name': 'Editor ID',
            'width': 'u32',
            'signed': False,
            'sig': 'EDID'
        })

    def test_parse_integer_s8_signed(self):
        """Test signed 8-bit integer."""
        result = self.ext._parse_integer("wbInteger(COND, 'Condition Value', itS8)")
        self.assertEqual(result, {
            'kind': 'integer',
            'name': 'Condition Value',
            'width': 's8',
            'signed': True,
            'sig': 'COND'
        })

    def test_parse_integer_s16(self):
        """Test signed 16-bit integer."""
        result = self.ext._parse_integer("wbInteger(SIGN, 'Signed Val', itS16)")
        self.assertEqual(result, {
            'kind': 'integer',
            'name': 'Signed Val',
            'width': 's16',
            'signed': True,
            'sig': 'SIGN'
        })

    def test_parse_integer_u8(self):
        """Test unsigned 8-bit integer."""
        result = self.ext._parse_integer("wbInteger(FLAG, 'Flags', itU8)")
        self.assertEqual(result, {
            'kind': 'integer',
            'name': 'Flags',
            'width': 'u8',
            'signed': False,
            'sig': 'FLAG'
        })

    def test_parse_integer_s64(self):
        """Test signed 64-bit integer."""
        result = self.ext._parse_integer("wbInteger(BIGV, 'Big Val', itS64)")
        self.assertEqual(result, {
            'kind': 'integer',
            'name': 'Big Val',
            'width': 's64',
            'signed': True,
            'sig': 'BIGV'
        })

    def test_parse_integer_lowercase_case_normalization(self):
        """Test lowercase type normalization (itu8 -> itU8)."""
        result = self.ext._parse_integer("wbInteger(TEST, 'Test', itu8)")
        self.assertEqual(result['width'], 'u8')
        self.assertEqual(result['signed'], False)

    def test_parse_integer_lowercase_signed_normalization(self):
        """Test lowercase signed type normalization (its32 -> itS32)."""
        result = self.ext._parse_integer("wbInteger(TEST, 'Test', its32)")
        self.assertEqual(result['width'], 's32')
        self.assertEqual(result['signed'], True)


class TestParseFloat(unittest.TestCase):
    """Tests for _parse_float method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_float_with_sig(self):
        """Test float with signature."""
        result = self.ext._parse_float("wbFloat(FLTV, 'Value')")
        self.assertEqual(result, {
            'kind': 'float',
            'name': 'Value',
            'sig': 'FLTV'
        })

    def test_parse_float_without_name(self):
        """Test float without explicit name (uses sig as name)."""
        result = self.ext._parse_float("wbFloat(WGHT)")
        self.assertEqual(result, {
            'kind': 'float',
            'name': 'WGHT',
            'sig': 'WGHT'
        })

    def test_parse_float_various_signatures(self):
        """Test float with different signature values."""
        result = self.ext._parse_float("wbFloat(DAMG, 'Damage Multiplier')")
        self.assertEqual(result['name'], 'Damage Multiplier')
        self.assertEqual(result['sig'], 'DAMG')


class TestParseFormID(unittest.TestCase):
    """Tests for _parse_formid method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_formid_with_refs(self):
        """Test formid with reference list."""
        result = self.ext._parse_formid("wbFormIDCk(NAME, 'Base', [WEAP, NULL])")
        self.assertEqual(result, {
            'kind': 'formid',
            'name': 'Base',
            'valid_refs': ['WEAP', 'NULL'],
            'sig': 'NAME'
        })

    def test_parse_formid_multiple_refs(self):
        """Test formid with multiple references."""
        result = self.ext._parse_formid("wbFormIDCk(CNTO, 'Item', [WEAP, ARMO])")
        self.assertEqual(result['valid_refs'], ['WEAP', 'ARMO'])
        self.assertEqual(result['sig'], 'CNTO')

    def test_parse_formid_no_refs(self):
        """Test formid without reference list (ck=True but no refs given)."""
        result = self.ext._parse_formid("wbFormIDCk(ITPR, 'Item Ref')")
        self.assertEqual(result, {
            'kind': 'formid',
            'name': 'Item Ref',
            'valid_refs': [],
            'sig': 'ITPR'
        })

    def test_parse_formid_ck_false(self):
        """Test formid with ck=False (no references extraction)."""
        result = self.ext._parse_formid("wbFormID(ITPR, 'Item Ref')", ck=False)
        self.assertEqual(result, {
            'kind': 'formid',
            'name': 'Item Ref',
            'valid_refs': [],
            'sig': 'ITPR'
        })

    def test_parse_formid_single_ref(self):
        """Test formid with single reference."""
        result = self.ext._parse_formid("wbFormIDCk(KYWD, 'Keyword', [KYWD])")
        self.assertEqual(result['valid_refs'], ['KYWD'])


class TestParseLString(unittest.TestCase):
    """Tests for _parse_lstring method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_lstring_with_sig_and_name(self):
        """Test lstring with signature and name."""
        result = self.ext._parse_lstring("wbLStringKC(DESC, 'Description', 0, cpTranslate)")
        self.assertEqual(result, {
            'kind': 'lstring',
            'sig': 'DESC',
            'name': 'Description'
        })

    def test_parse_lstring_full_form(self):
        """Test lstring in full translatable form."""
        result = self.ext._parse_lstring("wbLStringKC(FULL, 'Name', 0, cpTranslate)")
        self.assertEqual(result['kind'], 'lstring')
        self.assertEqual(result['sig'], 'FULL')
        self.assertEqual(result['name'], 'Name')

    def test_parse_lstring_various_sigs(self):
        """Test lstring with different signatures."""
        result = self.ext._parse_lstring("wbLStringKC(NNAM, 'Nickname')")
        self.assertEqual(result['sig'], 'NNAM')
        self.assertEqual(result['kind'], 'lstring')


class TestParseStruct(unittest.TestCase):
    """Tests for _parse_struct method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_struct_with_sig_and_fields(self):
        """Test struct with signature and multiple fields."""
        result = self.ext._parse_struct(
            "wbStruct(DATA, 'Data', [wbFloat('Weight'), wbInteger('Value', itU32)])"
        )
        self.assertEqual(result['kind'], 'struct')
        self.assertEqual(result['sig'], 'DATA')
        self.assertEqual(result['name'], 'Data')
        self.assertEqual(len(result['fields']), 2)
        self.assertEqual(result['fields'][0]['kind'], 'float')
        self.assertEqual(result['fields'][1]['kind'], 'integer')

    def test_parse_struct_single_field(self):
        """Test struct with a single field."""
        result = self.ext._parse_struct("wbStruct(OBND, 'Bounds', [wbInteger('X', itS16)])")
        self.assertEqual(result['sig'], 'OBND')
        self.assertEqual(len(result['fields']), 1)

    def test_parse_struct_nested_complexity(self):
        """Test struct parsing handles nested field list extraction."""
        result = self.ext._parse_struct(
            "wbStruct(DNAM, 'Data', [wbInteger('Type', itU8), wbFloat('Factor'), wbInteger('Flags', itU16)])"
        )
        self.assertEqual(result['sig'], 'DNAM')
        self.assertEqual(len(result['fields']), 3)
        # Verify field kinds
        kinds = [f['kind'] for f in result['fields']]
        self.assertEqual(kinds, ['integer', 'float', 'integer'])


class TestParseRStruct(unittest.TestCase):
    """Tests for _parse_rstruct method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_rstruct_with_members(self):
        """Test rstruct with members."""
        result = self.ext._parse_rstruct("wbRStruct('Model', [wbString(MODL, 'Model FileName')], [])")
        self.assertEqual(result['kind'], 'rstruct')
        self.assertEqual(result['name'], 'Model')
        self.assertEqual(len(result['members']), 1)
        self.assertEqual(result['members'][0]['kind'], 'string')

    def test_parse_rstruct_multiple_members(self):
        """Test rstruct with multiple members."""
        result = self.ext._parse_rstruct(
            "wbRStruct('Reference', [wbString(NAME, 'Name'), wbInteger(DATA, 'Value', itU32)])"
        )
        self.assertEqual(result['name'], 'Reference')
        self.assertEqual(len(result['members']), 2)


class TestParseRStructS(unittest.TestCase):
    """Tests for _parse_rstructS method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_rstructS_basic(self):
        """Test rstructS (array of rstruct) with basic members."""
        result = self.ext._parse_rstructS(
            "wbRStructS('Parts', 'Part', [wbString(ANAM, 'Name'), wbInteger(BNAM, 'Value', itU8)])"
        )
        self.assertEqual(result['kind'], 'rarray')
        self.assertEqual(result['name'], 'Parts')
        self.assertEqual(result['element']['kind'], 'rstruct')
        self.assertEqual(result['element']['name'], 'Part')
        self.assertEqual(len(result['element']['members']), 2)

    def test_parse_rstructS_name_deduplication(self):
        """Test rstructS deduplicates member names with (SIG) suffix."""
        result = self.ext._parse_rstructS(
            "wbRStructS('Group', 'Elem', [wbString(ANAM, 'Part'), wbInteger(BNAM, 'Part', itU8)])"
        )
        members = result['element']['members']
        # First member keeps original name
        self.assertEqual(members[0]['name'], 'Part')
        # Second member with same name gets (SIG) suffix
        self.assertEqual(members[1]['name'], 'Part (BNAM)')

    def test_parse_rstructS_different_names(self):
        """Test rstructS with distinct member names (no dedup needed)."""
        result = self.ext._parse_rstructS(
            "wbRStructS('Items', 'Item', [wbString(ANAM, 'Name'), wbInteger(BNAM, 'Count', itU32)])"
        )
        members = result['element']['members']
        self.assertEqual(members[0]['name'], 'Name')
        self.assertEqual(members[1]['name'], 'Count')


class TestParseRArray(unittest.TestCase):
    """Tests for _parse_rarray method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_rarray_with_formid_element(self):
        """Test rarray with formid element."""
        result = self.ext._parse_rarray("wbRArray('Items', wbFormIDCk(CNTO, 'Item', [WEAP, ARMO]))")
        self.assertEqual(result['kind'], 'rarray')
        self.assertEqual(result['name'], 'Items')
        self.assertEqual(result['element']['kind'], 'formid')
        self.assertEqual(result['element']['sig'], 'CNTO')

    def test_parse_rarray_with_integer_element(self):
        """Test rarray with integer element."""
        result = self.ext._parse_rarray("wbRArray('Counts', wbInteger(CNTR, 'Count', itU32))")
        self.assertEqual(result['element']['kind'], 'integer')
        self.assertEqual(result['element']['sig'], 'CNTR')


class TestParseArray(unittest.TestCase):
    """Tests for _parse_array method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_array_with_sig(self):
        """Test array with signature."""
        result = self.ext._parse_array("wbArray(KWDA, 'Keywords', wbFormIDCk('Keyword', [KYWD]))")
        self.assertEqual(result['kind'], 'array')
        self.assertEqual(result['sig'], 'KWDA')
        self.assertEqual(result['name'], 'Keywords')
        self.assertEqual(result['element']['kind'], 'formid')

    def test_parse_array_without_sig(self):
        """Test array without signature."""
        result = self.ext._parse_array("wbArray('Items', wbInteger('Item', itU8))")
        self.assertEqual(result['kind'], 'array')
        self.assertNotIn('sig', result)
        self.assertEqual(result['name'], 'Items')

    def test_parse_array_with_count_prefix(self):
        """Test array with count prefix (-1 indicates 4-byte prefix)."""
        result = self.ext._parse_array("wbArray(KWDA, 'Keywords', wbFormIDCk('Keyword', [KYWD]), -1)")
        self.assertIn('count', result)
        self.assertEqual(result['count'], {'count_prefix': 4})

    def test_parse_array_with_count_prefix_2byte(self):
        """Test array with 2-byte count prefix (-2)."""
        result = self.ext._parse_array("wbArray(TEST, 'Items', wbInteger('Item', itU16), -2)")
        self.assertEqual(result['count'], {'count_prefix': 2})

    def test_parse_array_with_count_prefix_1byte(self):
        """Test array with 1-byte count prefix (-4)."""
        result = self.ext._parse_array("wbArray(TEST, 'Items', wbInteger('Item', itU8), -4)")
        self.assertEqual(result['count'], {'count_prefix': 1})


class TestParseUnion(unittest.TestCase):
    """Tests for _parse_union method."""

    def setUp(self):
        """Create a fresh Extractor for each test."""
        self.ext = Extractor("", "")

    def test_parse_union_with_form_version_decider(self):
        """Test union with form_version decider."""
        result = self.ext._parse_union(
            "wbUnion(DNAM, 'Data', wbFormVersionDecider(43), "
            "[wbInteger('Version A', itU32), wbInteger('Version B', itU32)])"
        )
        self.assertEqual(result['kind'], 'union')
        self.assertEqual(result['sig'], 'DNAM')
        self.assertEqual(result['name'], 'Data')
        self.assertIn('decider', result)
        self.assertIn('form_version', result['decider'])
        self.assertEqual(result['decider']['form_version']['min'], 43)
        self.assertIsNone(result['decider']['form_version']['max'])
        self.assertEqual(len(result['variants']), 2)

    def test_parse_union_with_form_version_min_max(self):
        """Test union with form_version decider having min and max."""
        result = self.ext._parse_union(
            "wbUnion('Data', wbFormVersionDecider(40, 50), "
            "[wbInteger('Type A', itU32), wbInteger('Type B', itU32)])"
        )
        self.assertEqual(result['decider']['form_version']['min'], 40)
        self.assertEqual(result['decider']['form_version']['max'], 50)

    def test_parse_union_with_record_size_decider(self):
        """Test union with record_size decider."""
        result = self.ext._parse_union(
            "wbUnion('Data', wbRecordSizeDecider(10), "
            "[wbInteger('Short', itU16), wbInteger('Long', itU32)])"
        )
        self.assertEqual(result['kind'], 'union')
        self.assertEqual(result['name'], 'Data')
        self.assertIn('payload_size', result['decider'])
        self.assertEqual(result['decider']['default_variant'], 1)
        # Check that payload_size map has keys for 0-9
        self.assertIn('0', result['decider']['payload_size'])
        self.assertIn('9', result['decider']['payload_size'])

    def test_parse_union_multiple_variants(self):
        """Test union with multiple variant options."""
        result = self.ext._parse_union(
            "wbUnion(TNAM, 'Type', wbFormVersionDecider(50), "
            "[wbInteger('V1', itU8), wbInteger('V2', itU16), wbInteger('V3', itU32)])"
        )
        self.assertEqual(len(result['variants']), 3)
        # Verify each variant is properly decoded
        self.assertEqual(result['variants'][0]['width'], 'u8')
        self.assertEqual(result['variants'][1]['width'], 'u16')
        self.assertEqual(result['variants'][2]['width'], 'u32')


if __name__ == '__main__':
    unittest.main()
