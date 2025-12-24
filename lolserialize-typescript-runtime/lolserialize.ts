export class Buf {
  data: Uint8Array;
  offset: number;

  constructor(data: Uint8Array, offset = 0) {
    this.data = data;
    this.offset = offset;
  }
}

export function encodeU8(value: number, buf: Uint8Array[]): void {
  buf.push(new Uint8Array([value]));
}

export function decodeU8(buf: Buf): number {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value;
}

export function encodeI8(value: number, buf: Uint8Array[]): void {
  buf.push(new Uint8Array([value & 0xff]));
}

export function decodeI8(buf: Buf): number {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value << 24 >> 24;
}

export function encodeBool(value: boolean, buf: Uint8Array[]): void {
  buf.push(new Uint8Array([value ? 1 : 0]));
}

export function decodeBool(buf: Buf): boolean {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value !== 0;
}

export function encodeVarint(value: number, buf: Uint8Array[]): void {
  const bytes: number[] = [];
  let v = Math.floor(value);

  while (v > 0x7f) {
    bytes.push((v & 0x7f) | 0x80);
    v = Math.floor(v / 128);
  }
  bytes.push(v & 0x7f);

  buf.push(new Uint8Array(bytes));
}

export function decodeVarint(buf: Buf): number {
  let result = 0;
  let shift = 0;

  while (true) {
    const byte = buf.data[buf.offset];
    buf.offset += 1;

    result = result + ((byte & 0x7f) * Math.pow(2, shift));

    if ((byte & 0x80) === 0) {
      break;
    }

    shift += 7;
  }

  return result;
}

export function encodeU16(value: number, buf: Uint8Array[]): void {
  encodeVarint(value, buf);
}

export function decodeU16(buf: Buf): number {
  return Number(decodeVarint(buf));
}

export function encodeU32(value: number, buf: Uint8Array[]): void {
  encodeVarint(value, buf);
}

export function decodeU32(buf: Buf): number {
  return Number(decodeVarint(buf));
}

export function encodeU64(value: number, buf: Uint8Array[]): void {
  encodeVarint(value, buf);
}

export function decodeU64(buf: Buf): number {
  return decodeVarint(buf);
}

export function zigzagEncode(value: number): number {
  // Use >>> 0 to ensure result is treated as unsigned 32-bit
  return ((value << 1) ^ (value >> 31)) >>> 0;
}

export function zigzagDecode(value: number): number {
  return (value >>> 1) ^ (-(value & 1));
}

export function encodeI16(value: number, buf: Uint8Array[]): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI16(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeI32(value: number, buf: Uint8Array[]): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI32(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeI64(value: number, buf: Uint8Array[]): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI64(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeF32(value: number, buf: Uint8Array[]): void {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setFloat32(0, value, true);
  buf.push(bytes);
}

export function decodeF32(buf: Buf): number {
  const value = new DataView(buf.data.buffer, buf.data.byteOffset + buf.offset).getFloat32(0, true);
  buf.offset += 4;
  return value;
}

export function encodeF64(value: number, buf: Uint8Array[]): void {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setFloat64(0, value, true);
  buf.push(bytes);
}

export function decodeF64(buf: Buf): number {
  const value = new DataView(buf.data.buffer, buf.data.byteOffset + buf.offset).getFloat64(0, true);
  buf.offset += 8;
  return value;
}

export function encodeString(value: string, buf: Uint8Array[]): void {
  const bytes = new TextEncoder().encode(value);
  encodeVarint(bytes.length, buf);
  buf.push(bytes);
}

export function decodeString(buf: Buf): string {
  const len = Number(decodeVarint(buf));
  const bytes = buf.data.slice(buf.offset, buf.offset + len);
  buf.offset += len;
  return new TextDecoder().decode(bytes);
}

export function encodeBytes(value: Uint8Array, buf: Uint8Array[]): void {
  encodeVarint(value.length, buf);
  buf.push(value);
}

export function decodeBytes(buf: Buf, len: number): Uint8Array {
  const bytes = buf.data.slice(buf.offset, buf.offset + len);
  buf.offset += len;
  return bytes;
}

export function encodeTag(index: number, wireType: number, buf: Uint8Array[]): void {
  const tag = (index << 3) | wireType;
  encodeVarint(tag, buf);
}

export function decodeTag(buf: Buf): { index: number; wireType: number } {
  const tag = Number(decodeVarint(buf));
  return {
    index: tag >> 3,
    wireType: tag & 0x7,
  };
}

export function flattenBuf(bufs: Uint8Array[]): Uint8Array {
  const totalLen = bufs.reduce((sum, b) => sum + b.length, 0);
  const result = new Uint8Array(totalLen);
  let offset = 0;
  for (const b of bufs) {
    result.set(b, offset);
    offset += b.length;
  }
  return result;
}

export function encodePresenceBits(bits: boolean[], buf: Uint8Array[]): void {
  const bytes = new Uint8Array(Math.ceil(bits.length / 8));
  for (let i = 0; i < bits.length; i++) {
    if (bits[i]) {
      bytes[i >> 3] |= 1 << (i & 7);
    }
  }
  buf.push(bytes);
}

export function decodePresenceBits(buf: Buf, count: number): boolean[] {
  const byteCount = Math.ceil(count / 8);
  const bytes = buf.data.slice(buf.offset, buf.offset + byteCount);
  buf.offset += byteCount;

  const bits: boolean[] = [];
  for (let i = 0; i < count; i++) {
    bits.push((bytes[i >> 3] & (1 << (i & 7))) !== 0);
  }
  return bits;
}
