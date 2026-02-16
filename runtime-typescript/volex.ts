export class Buf {
  data: Uint8Array;
  offset: number;

  constructor(data: Uint8Array, offset = 0) {
    this.data = data;
    this.offset = offset;
  }
}

export class WriteBuf {
  private buffer: Uint8Array;
  private position: number;

  constructor(initialCapacity = 256) {
    this.buffer = new Uint8Array(initialCapacity);
    this.position = 0;
  }

  private ensureCapacity(additionalBytes: number): void {
    const required = this.position + additionalBytes;
    if (required <= this.buffer.length) {
      return;
    }

    // Double the buffer size until it fits, ensuring we grow efficiently
    let newCapacity = this.buffer.length * 2;
    while (newCapacity < required) {
      newCapacity *= 2;
    }

    const newBuffer = new Uint8Array(newCapacity);
    newBuffer.set(this.buffer);
    this.buffer = newBuffer;
  }

  push(data: Uint8Array): void {
    this.ensureCapacity(data.length);
    this.buffer.set(data, this.position);
    this.position += data.length;
  }

  pushByte(byte: number): void {
    this.ensureCapacity(1);
    this.buffer[this.position] = byte;
    this.position += 1;
  }

  toUint8Array(): Uint8Array {
    return this.buffer.slice(0, this.position);
  }
}

export function encodeU8(value: number, buf: WriteBuf): void {
  buf.pushByte(value);
}

export function decodeU8(buf: Buf): number {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value;
}

export function encodeI8(value: number, buf: WriteBuf): void {
  buf.pushByte(value & 0xff);
}

export function decodeI8(buf: Buf): number {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value << 24 >> 24;
}

export function encodeBool(value: boolean, buf: WriteBuf): void {
  buf.pushByte(value ? 1 : 0);
}

export function decodeBool(buf: Buf): boolean {
  const value = buf.data[buf.offset];
  buf.offset += 1;
  return value !== 0;
}

export function encodeVarint(value: number, buf: WriteBuf): void {
  let v = Math.floor(value);

  while (v > 0x7f) {
    buf.pushByte((v & 0x7f) | 0x80);
    v = Math.floor(v / 128);
  }
  buf.pushByte(v & 0x7f);
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

export function encodeU16(value: number, buf: WriteBuf): void {
  encodeVarint(value, buf);
}

export function decodeU16(buf: Buf): number {
  return Number(decodeVarint(buf));
}

export function encodeU32(value: number, buf: WriteBuf): void {
  encodeVarint(value, buf);
}

export function decodeU32(buf: Buf): number {
  return Number(decodeVarint(buf));
}

export function encodeU64(value: number, buf: WriteBuf): void {
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

export function encodeI16(value: number, buf: WriteBuf): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI16(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeI32(value: number, buf: WriteBuf): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI32(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeI64(value: number, buf: WriteBuf): void {
  encodeVarint(zigzagEncode(value), buf);
}

export function decodeI64(buf: Buf): number {
  return zigzagDecode(decodeVarint(buf));
}

export function encodeF32(value: number, buf: WriteBuf): void {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setFloat32(0, value, true);
  buf.push(bytes);
}

export function decodeF32(buf: Buf): number {
  const value = new DataView(buf.data.buffer, buf.data.byteOffset + buf.offset).getFloat32(0, true);
  buf.offset += 4;
  return value;
}

export function encodeF64(value: number, buf: WriteBuf): void {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setFloat64(0, value, true);
  buf.push(bytes);
}

export function decodeF64(buf: Buf): number {
  const value = new DataView(buf.data.buffer, buf.data.byteOffset + buf.offset).getFloat64(0, true);
  buf.offset += 8;
  return value;
}

export function encodeString(value: string, buf: WriteBuf): void {
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

export function encodeBytes(value: Uint8Array, buf: WriteBuf): void {
  encodeVarint(value.length, buf);
  buf.push(value);
}

export function decodeBytes(buf: Buf, len: number): Uint8Array {
  const bytes = buf.data.slice(buf.offset, buf.offset + len);
  buf.offset += len;
  return bytes;
}

export function encodeTag(index: number, wireType: number, buf: WriteBuf): void {
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

export function encodePresenceBits(bits: boolean[], buf: WriteBuf): void {
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
