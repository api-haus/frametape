// SPDX-License-Identifier: MIT
// Stored ZIP: video stays encoded; frame/event streams remain inspectable without this library.
(() => {
    const utf8 = new TextEncoder();
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; table[n] = c; }
    const crc32 = (bytes) => { let c = 0xffffffff; for (const b of bytes) c = table[(c ^ b) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
    const safe = (name) => typeof name === 'string' && name.length && !/[\\:\0]/.test(name) && name.split('/').every((p) => p && p !== '.' && p !== '..');
    const lines = (values) => utf8.encode(values.map((v) => JSON.stringify(v) + '\n').join(''));

    async function create({ metadata = {}, frames = [], events = [], channels = [], attachments = [], annotations = null }) {
        if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) throw new TypeError('metadata must be an object');
        const registered = new Map(channels.map((c) => [c.id, c]));
        const register = (id, schema) => {
            if (typeof id !== 'string' || !id.length) throw new TypeError('channel id required');
            if (!registered.has(id)) registered.set(id, { id, kind: id.split('.')[0], schema });
        };
        let last = 0, duration = 0;
        for (const [i, f] of frames.entries()) {
            if (f.index !== i || !Number.isSafeInteger(f.timestamp_ns) || f.timestamp_ns < last || !Number.isSafeInteger(f.duration_ns) || f.duration_ns < 0 || !(f.video_frame === null || Number.isSafeInteger(f.video_frame) && f.video_frame >= 0)) throw new RangeError('invalid frame clock or picture mapping');
            last = f.timestamp_ns; duration = Math.max(duration, f.timestamp_ns + f.duration_ns);
        }
        for (const e of events) {
            if (!(e.frame === null || Number.isSafeInteger(e.frame) && e.frame >= 0 && e.frame < frames.length) || !Number.isSafeInteger(e.timestamp_ns) || e.timestamp_ns < 0) throw new RangeError('invalid event frame or timestamp');
            register(e.channel, 'application/json');
        }
        const members = [], descriptors = [];
        for (const a of attachments) {
            if (!safe(a.name)) throw new TypeError('unsafe attachment name');
            const data = a.data instanceof Blob ? new Uint8Array(await a.data.arrayBuffer()) : a.data;
            if (!(data instanceof Uint8Array)) throw new TypeError('attachment data must be bytes or Blob');
            register(a.channel, a.media_type);
            const path = 'attachments/' + a.name;
            members.push([path, data]); descriptors.push({ channel: a.channel, path, media_type: a.media_type, bytes: data.length });
        }
        const manifest = { format: 'frametape', version: 1, id: crypto.randomUUID(), created_unix_ns: Date.now() * 1e6, metadata, channels: [...registered.values()], attachments: descriptors, frame_count: frames.length, event_count: events.length, duration_ns: duration };
        members.unshift(['manifest.json', utf8.encode(JSON.stringify(manifest))], ['frames.jsonl', lines(frames)], ['events.jsonl', lines(events)]);
        if (annotations) members.push(['annotations.json', utf8.encode(JSON.stringify(annotations))]);
        return zip(members);
    }

    function zip(members) {
        const local = [], central = [], names = new Set();
        let offset = 0, centralSize = 0;
        for (const [name, bytes] of members) {
            if (!safe(name) || names.has(name)) throw new TypeError('unsafe or duplicate ZIP member');
            names.add(name);
            const label = utf8.encode(name), crc = crc32(bytes);
            if (label.length > 65535 || bytes.length > 0xffffffff || offset + bytes.length + label.length + 30 > 0xffffffff) throw new RangeError('browser ZIP requires a recording below 4 GiB');
            const l = new Uint8Array(30), v = new DataView(l.buffer);
            v.setUint32(0, 0x04034b50, true); v.setUint16(4, 20, true); v.setUint16(6, 0x800, true);
            v.setUint32(14, crc, true); v.setUint32(18, bytes.length, true); v.setUint32(22, bytes.length, true); v.setUint16(26, label.length, true);
            local.push(l, label, bytes);
            const c = new Uint8Array(46), d = new DataView(c.buffer);
            d.setUint32(0, 0x02014b50, true); d.setUint16(4, 20, true); d.setUint16(6, 20, true); d.setUint16(8, 0x800, true);
            d.setUint32(16, crc, true); d.setUint32(20, bytes.length, true); d.setUint32(24, bytes.length, true); d.setUint16(28, label.length, true); d.setUint32(42, offset, true);
            central.push(c, label); centralSize += c.length + label.length; offset += l.length + label.length + bytes.length;
        }
        if (members.length > 65535 || offset + centralSize + 22 > 0xffffffff) throw new RangeError('browser ZIP limits exceeded');
        const end = new Uint8Array(22), e = new DataView(end.buffer);
        e.setUint32(0, 0x06054b50, true); e.setUint16(8, members.length, true); e.setUint16(10, members.length, true); e.setUint32(12, centralSize, true); e.setUint32(16, offset, true);
        return new Blob([...local, ...central, end], { type: 'application/zip' });
    }
    function download(blob, name) {
        const link = document.createElement('a'), url = URL.createObjectURL(blob);
        link.href = url; link.download = name; link.click();
        setTimeout(() => URL.revokeObjectURL(url), 60000);
    }
    globalThis.FrametapeBundle = { create, download };
})();
