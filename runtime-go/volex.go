// Package volex provides runtime encoding/decoding support.
package volex

import (
	"encoding/binary"
	"errors"
	"math"
)

// Wire type constants for encoding/decoding.
const (
	WireFixed8  byte = 0
	WireVarint  byte = 1
	WireFixed32 byte = 2
	WireFixed64 byte = 3
	WireBytes   byte = 4
	WireMessage byte = 5
	WireUnion   byte = 6
	WireUnit    byte = 7
)

// Common errors
var (
	ErrUnexpectedEOF       = errors.New("unexpected end of input")
	ErrInvalidUTF8         = errors.New("invalid UTF-8 in string")
	ErrInvalidBool         = errors.New("invalid bool value (not 0 or 1)")
	ErrUnknownEnumVariant  = errors.New("unknown enum variant")
	ErrUnknownUnionVariant = errors.New("unknown union variant")
	ErrLeb128Overflow      = errors.New("LEB128 overflow")
	ErrInvalidWireType     = errors.New("invalid wire type")
	ErrInvalidLength       = errors.New("invalid length (not a multiple of element size)")
)

// ============================================================================
// LEB128 encoding/decoding
// ============================================================================

// EncodeLEB128 encodes an unsigned 64-bit integer as LEB128.
func EncodeLEB128(value uint64, buf *[]byte) {
	for {
		b := byte(value & 0x7F)
		value >>= 7
		if value != 0 {
			b |= 0x80
		}
		*buf = append(*buf, b)
		if value == 0 {
			break
		}
	}
}

// DecodeLEB128 decodes an unsigned LEB128 integer.
func DecodeLEB128(buf *[]byte) (uint64, error) {
	var result uint64
	var shift uint
	for {
		if len(*buf) == 0 {
			return 0, ErrUnexpectedEOF
		}
		b := (*buf)[0]
		*buf = (*buf)[1:]

		if shift >= 64 {
			return 0, ErrLeb128Overflow
		}

		result |= uint64(b&0x7F) << shift
		if b&0x80 == 0 {
			return result, nil
		}
		shift += 7
	}
}

// ZigzagEncode encodes a signed integer using zigzag encoding.
func ZigzagEncode(n int64) uint64 {
	return uint64((n << 1) ^ (n >> 63))
}

// ZigzagDecode decodes a zigzag-encoded unsigned integer to signed.
func ZigzagDecode(n uint64) int64 {
	return int64(n>>1) ^ -int64(n&1)
}

// ============================================================================
// Primitive type encoding/decoding
// ============================================================================

// EncodeBool encodes a boolean value.
func EncodeBool(v bool, buf *[]byte) {
	if v {
		*buf = append(*buf, 1)
	} else {
		*buf = append(*buf, 0)
	}
}

// DecodeBool decodes a boolean value.
func DecodeBool(buf *[]byte) (bool, error) {
	if len(*buf) == 0 {
		return false, ErrUnexpectedEOF
	}
	b := (*buf)[0]
	*buf = (*buf)[1:]
	switch b {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, ErrInvalidBool
	}
}

// EncodeU8 encodes a uint8.
func EncodeU8(v uint8, buf *[]byte) {
	*buf = append(*buf, v)
}

// DecodeU8 decodes a uint8.
func DecodeU8(buf *[]byte) (uint8, error) {
	if len(*buf) == 0 {
		return 0, ErrUnexpectedEOF
	}
	v := (*buf)[0]
	*buf = (*buf)[1:]
	return v, nil
}

// EncodeI8 encodes an int8.
func EncodeI8(v int8, buf *[]byte) {
	*buf = append(*buf, byte(v))
}

// DecodeI8 decodes an int8.
func DecodeI8(buf *[]byte) (int8, error) {
	if len(*buf) == 0 {
		return 0, ErrUnexpectedEOF
	}
	v := (*buf)[0]
	*buf = (*buf)[1:]
	return int8(v), nil
}

// EncodeU16 encodes a uint16 using varint.
func EncodeU16(v uint16, buf *[]byte) {
	EncodeLEB128(uint64(v), buf)
}

// DecodeU16 decodes a uint16 varint.
func DecodeU16(buf *[]byte) (uint16, error) {
	v, err := DecodeLEB128(buf)
	return uint16(v), err
}

// EncodeU32 encodes a uint32 using varint.
func EncodeU32(v uint32, buf *[]byte) {
	EncodeLEB128(uint64(v), buf)
}

// DecodeU32 decodes a uint32 varint.
func DecodeU32(buf *[]byte) (uint32, error) {
	v, err := DecodeLEB128(buf)
	return uint32(v), err
}

// EncodeU64 encodes a uint64 using varint.
func EncodeU64(v uint64, buf *[]byte) {
	EncodeLEB128(v, buf)
}

// DecodeU64 decodes a uint64 varint.
func DecodeU64(buf *[]byte) (uint64, error) {
	return DecodeLEB128(buf)
}

// EncodeI16 encodes an int16 using zigzag varint.
func EncodeI16(v int16, buf *[]byte) {
	EncodeLEB128(ZigzagEncode(int64(v)), buf)
}

// DecodeI16 decodes an int16 zigzag varint.
func DecodeI16(buf *[]byte) (int16, error) {
	v, err := DecodeLEB128(buf)
	if err != nil {
		return 0, err
	}
	return int16(ZigzagDecode(v)), nil
}

// EncodeI32 encodes an int32 using zigzag varint.
func EncodeI32(v int32, buf *[]byte) {
	EncodeLEB128(ZigzagEncode(int64(v)), buf)
}

// DecodeI32 decodes an int32 zigzag varint.
func DecodeI32(buf *[]byte) (int32, error) {
	v, err := DecodeLEB128(buf)
	if err != nil {
		return 0, err
	}
	return int32(ZigzagDecode(v)), nil
}

// EncodeI64 encodes an int64 using zigzag varint.
func EncodeI64(v int64, buf *[]byte) {
	EncodeLEB128(ZigzagEncode(v), buf)
}

// DecodeI64 decodes an int64 zigzag varint.
func DecodeI64(buf *[]byte) (int64, error) {
	v, err := DecodeLEB128(buf)
	if err != nil {
		return 0, err
	}
	return ZigzagDecode(v), nil
}

// EncodeF32 encodes a float32 as little-endian.
func EncodeF32(v float32, buf *[]byte) {
	bits := math.Float32bits(v)
	*buf = append(*buf,
		byte(bits),
		byte(bits>>8),
		byte(bits>>16),
		byte(bits>>24),
	)
}

// DecodeF32 decodes a little-endian float32.
func DecodeF32(buf *[]byte) (float32, error) {
	if len(*buf) < 4 {
		return 0, ErrUnexpectedEOF
	}
	bits := binary.LittleEndian.Uint32(*buf)
	*buf = (*buf)[4:]
	return math.Float32frombits(bits), nil
}

// EncodeF64 encodes a float64 as little-endian.
func EncodeF64(v float64, buf *[]byte) {
	bits := math.Float64bits(v)
	*buf = append(*buf,
		byte(bits),
		byte(bits>>8),
		byte(bits>>16),
		byte(bits>>24),
		byte(bits>>32),
		byte(bits>>40),
		byte(bits>>48),
		byte(bits>>56),
	)
}

// DecodeF64 decodes a little-endian float64.
func DecodeF64(buf *[]byte) (float64, error) {
	if len(*buf) < 8 {
		return 0, ErrUnexpectedEOF
	}
	bits := binary.LittleEndian.Uint64(*buf)
	*buf = (*buf)[8:]
	return math.Float64frombits(bits), nil
}

// EncodeString encodes a string with length prefix.
func EncodeString(v string, buf *[]byte) {
	EncodeLEB128(uint64(len(v)), buf)
	*buf = append(*buf, v...)
}

// DecodeString decodes a length-prefixed string.
func DecodeString(buf *[]byte) (string, error) {
	length, err := DecodeLEB128(buf)
	if err != nil {
		return "", err
	}
	if uint64(len(*buf)) < length {
		return "", ErrUnexpectedEOF
	}
	s := string((*buf)[:length])
	*buf = (*buf)[length:]
	return s, nil
}

// EncodeStringDelimited encodes a string without length prefix (caller provides length).
func EncodeStringDelimited(v string, buf *[]byte) {
	*buf = append(*buf, v...)
}

// DecodeStringDelimited decodes a string with known length.
func DecodeStringDelimited(length int, buf *[]byte) (string, error) {
	if len(*buf) < length {
		return "", ErrUnexpectedEOF
	}
	s := string((*buf)[:length])
	*buf = (*buf)[length:]
	return s, nil
}

// EncodeBytes encodes a byte slice with length prefix.
func EncodeBytes(v []byte, buf *[]byte) {
	EncodeLEB128(uint64(len(v)), buf)
	*buf = append(*buf, v...)
}

// DecodeBytes decodes a length-prefixed byte slice.
func DecodeBytes(buf *[]byte) ([]byte, error) {
	length, err := DecodeLEB128(buf)
	if err != nil {
		return nil, err
	}
	if uint64(len(*buf)) < length {
		return nil, ErrUnexpectedEOF
	}
	bytes := make([]byte, length)
	copy(bytes, (*buf)[:length])
	*buf = (*buf)[length:]
	return bytes, nil
}

// EncodeBytesDelimited encodes bytes without length prefix.
func EncodeBytesDelimited(v []byte, buf *[]byte) {
	*buf = append(*buf, v...)
}

// DecodeBytesDelimited decodes bytes with known length.
func DecodeBytesDelimited(length int, buf *[]byte) ([]byte, error) {
	if len(*buf) < length {
		return nil, ErrUnexpectedEOF
	}
	bytes := make([]byte, length)
	copy(bytes, (*buf)[:length])
	*buf = (*buf)[length:]
	return bytes, nil
}

// ============================================================================
// Message encoding/decoding helpers
// ============================================================================

// EncodeTag encodes a message field tag (index and wire type).
func EncodeTag(index uint32, wireType byte, buf *[]byte) {
	tag := (uint64(index) << 3) | uint64(wireType)
	EncodeLEB128(tag, buf)
}

// DecodeTag decodes a message field tag, returning (index, wireType, hasMore).
// Returns hasMore=false if the terminator (0) is encountered.
func DecodeTag(buf *[]byte) (uint32, byte, bool, error) {
	tag, err := DecodeLEB128(buf)
	if err != nil {
		return 0, 0, false, err
	}
	if tag == 0 {
		return 0, 0, false, nil
	}
	wireType := byte(tag & 0x7)
	index := uint32(tag >> 3)
	return index, wireType, true, nil
}

// EncodeMessageEnd encodes a message terminator.
func EncodeMessageEnd(buf *[]byte) {
	*buf = append(*buf, 0)
}

// SkipByWireType skips a field value based on wire type.
func SkipByWireType(wireType byte, buf *[]byte) error {
	switch wireType {
	case WireFixed8:
		if len(*buf) < 1 {
			return ErrUnexpectedEOF
		}
		*buf = (*buf)[1:]
		return nil
	case WireVarint:
		_, err := DecodeLEB128(buf)
		return err
	case WireFixed32:
		if len(*buf) < 4 {
			return ErrUnexpectedEOF
		}
		*buf = (*buf)[4:]
		return nil
	case WireFixed64:
		if len(*buf) < 8 {
			return ErrUnexpectedEOF
		}
		*buf = (*buf)[8:]
		return nil
	case WireBytes:
		length, err := DecodeLEB128(buf)
		if err != nil {
			return err
		}
		if uint64(len(*buf)) < length {
			return ErrUnexpectedEOF
		}
		*buf = (*buf)[length:]
		return nil
	case WireMessage:
		// Read fields until terminator
		for {
			_, wt, hasMore, err := DecodeTag(buf)
			if err != nil {
				return err
			}
			if !hasMore {
				break
			}
			if err := SkipByWireType(wt, buf); err != nil {
				return err
			}
		}
		return nil
	case WireUnion:
		// Read tag and skip payload
		tag, err := DecodeLEB128(buf)
		if err != nil {
			return err
		}
		wt := byte(tag & 0x7)
		return SkipByWireType(wt, buf)
	case WireUnit:
		// No payload to skip
		return nil
	default:
		return ErrInvalidWireType
	}
}

// ============================================================================
// Struct presence bits helpers
// ============================================================================

// EncodePresenceBits encodes presence bits for optional fields.
func EncodePresenceBits(bits []bool, buf *[]byte) {
	numBytes := (len(bits) + 7) / 8
	for byteIdx := 0; byteIdx < numBytes; byteIdx++ {
		var b byte
		for bitIdx := 0; bitIdx < 8; bitIdx++ {
			idx := byteIdx*8 + bitIdx
			if idx < len(bits) && bits[idx] {
				b |= 1 << bitIdx
			}
		}
		*buf = append(*buf, b)
	}
}

// DecodePresenceBits decodes presence bits for optional fields.
func DecodePresenceBits(count int, buf *[]byte) ([]bool, error) {
	numBytes := (count + 7) / 8
	if len(*buf) < numBytes {
		return nil, ErrUnexpectedEOF
	}
	result := make([]bool, count)
	for i := 0; i < count; i++ {
		byteIdx := i / 8
		bitIdx := i % 8
		result[i] = ((*buf)[byteIdx]>>bitIdx)&1 == 1
	}
	*buf = (*buf)[numBytes:]
	return result, nil
}
