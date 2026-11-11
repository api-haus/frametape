// SPDX-License-Identifier: MIT
// WebGPU producer: logical frame IDs in, Annex B H.264 and an explicit picture map out.
globalThis.FrametapeVideo = { create(rec, options = {}) {
	const MAX_WIDTH = options.maxWidth ?? 1280, RING = 8;
	if (!Number.isSafeInteger(MAX_WIDTH) || MAX_WIDTH < 0) throw new RangeError("maxWidth must be a nonnegative integer");
	let state = null, blit = null;
	const pipeline = (device) => {
		if (blit && blit.device === device) return blit;
		const module = device.createShaderModule({ code: [
			'@group(0) @binding(0) var src: texture_2d<f32>;',
			'@group(0) @binding(1) var smp: sampler;',
			'struct V { @builtin(position) p: vec4f, @location(0) uv: vec2f };',
			'@vertex fn vs(@builtin(vertex_index) i: u32) -> V {',
			'	let xy = vec2f(f32((i << 1u) & 2u), f32(i & 2u));',
			'	return V(vec4f(xy * 2.0 - 1.0, 0.0, 1.0), vec2f(xy.x, 1.0 - xy.y));',
			'}',
			'@fragment fn fs(v: V) -> @location(0) vec4f { return textureSampleLevel(src, smp, v.uv, 0.0); }'].join('\n') });
		blit = { device, sampler: device.createSampler({ minFilter: 'linear', magFilter: 'linear' }),
			pipeline: device.createRenderPipeline({ layout: 'auto', vertex: { module, entryPoint: 'vs' },
				fragment: { module, entryPoint: 'fs', targets: [{ format: 'rgba8unorm' }] }, primitive: { topology: 'triangle-list' } }) };
		return blit;
	};
	// `canvas` is the frame's canvas texture once the engine has drawn it, or null to repeat the previous picture.
	const capture = (s, index, canvas) => {
		const timestamp = Math.round(index * 1e6 / 60);
		let repeated = !canvas;
		let picture = Promise.resolve(null);
		const config = canvas && canvas.context.getConfiguration();
		if (config && (config.usage & GPUTextureUsage.TEXTURE_BINDING) && !s.failed) {
			const { texture } = canvas, device = config.device;
			const width = (MAX_WIDTH ? Math.min(MAX_WIDTH, texture.width) : texture.width) & ~1, height = Math.round(texture.height * width / texture.width) & ~1;
			if (!s.encoder) {
				s.size = width + 'x' + height; s.width = width; s.height = height;
				try {
					s.encoder = new VideoEncoder({ output: (chunk) => { const b = new Uint8Array(chunk.byteLength); chunk.copyTo(b); s.chunks.push(b); s.outputTimes.push(chunk.timestamp); },
						error: (error) => { rec.note('video: encoder failed: ' + error); s.failed = true; } });
					s.encoder.configure({ codec: 'avc1.64002A', width, height, framerate: 60, bitrate: options.bitrate ?? 6e6, hardwareAcceleration: options.hardwareAcceleration ?? 'no-preference', latencyMode: 'realtime', avc: { format: 'annexb' } });
				} catch (error) { rec.note('video: no H.264 encoder: ' + error); s.failed = true; }
			} else if (s.size !== width + 'x' + height) {
				rec.note('video: the canvas resized to ' + texture.width + 'x' + texture.height + '; the video ends at take frame ' + index);
				s.failed = true;
			}
			const row = Math.ceil(width * 4 / 256) * 256;
			let slot = !s.failed && s.ring.find((r) => !r.busy);
			if (!s.failed && !slot && s.ring.length < RING) {
				slot = { busy: false, buffer: device.createBuffer({ size: row * height, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ }),
					target: device.createTexture({ size: [width, height], format: 'rgba8unorm', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC }) };
				s.ring.push(slot);
			}
			if (slot) {
				slot.busy = true;
				const b = pipeline(device), encoder = device.createCommandEncoder();
				const pass = encoder.beginRenderPass({ colorAttachments: [{ view: slot.target.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] }] });
				pass.setPipeline(b.pipeline);
				pass.setBindGroup(0, device.createBindGroup({ layout: b.pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: texture.createView({ format: texture.format }) }, { binding: 1, resource: b.sampler }] }));
				pass.draw(3);
				pass.end();
				encoder.copyTextureToBuffer({ texture: slot.target }, { buffer: slot.buffer, bytesPerRow: row }, [width, height]);
				device.queue.submit([encoder.finish()]);
				picture = slot.buffer.mapAsync(GPUMapMode.READ).then(() => {
					try { return new VideoFrame(new Uint8Array(slot.buffer.getMappedRange()), { format: 'RGBX', codedWidth: width, codedHeight: height, timestamp, layout: [{ offset: 0, stride: row }] }); } finally { slot.buffer.unmap(); }
				}, () => null).finally(() => { slot.busy = false; });
			} else if (!s.failed) rec.note('video: take frame ' + index + ' repeats the previous picture (every readback buffer busy)');
		}
		s.chain = s.chain.then(() => picture).then((frame) => {
			if (s.failed || !s.encoder) { if (frame) frame.close(); s.mapping.push({ source_frame: index, video_frame: null, status: 'encoder_unavailable' }); return; }
			if (!frame && s.last) { repeated = true; frame = new VideoFrame(s.last, { timestamp }); }
			if (!frame) { s.mapping.push({ source_frame: index, video_frame: null, status: 'no_picture' }); return; }
			if (s.encoder.encodeQueueSize >= RING) { frame.close(); s.mapping.push({ source_frame: index, video_frame: s.frames ? s.frames - 1 : null, status: 'encoder_busy' }); return; }
			try { s.encoder.encode(frame, { keyFrame: s.frames % 30 === 0 }); } catch (error) { frame.close(); s.failed = true; rec.note('video encode failed: ' + error); s.mapping.push({ source_frame: index, video_frame: null, status: 'encoder_failed' }); return; }
			s.mapping.push({ source_frame: index, video_frame: s.frames, status: repeated ? 'repeated' : 'captured' });
			s.acceptedTimes.push(timestamp); s.frames++;
			if (s.last) s.last.close();
			s.last = frame;
		});
	};
	return {
		begin() {
			if (state) throw new Error('already recording');
			state = self.VideoEncoder && self.VideoFrame ? { encoder: null, failed: false, mapping: [], acceptedTimes: [], outputTimes: [], ring: [], chain: Promise.resolve(), last: null, frames: 0, chunks: [], pending: null } : null;
			if (!state) { rec.note('video: this browser has no WebCodecs'); return; }
			// A canvas configured before the hooks above lacks the usage the scaler samples; this runs between frames.
			const config = rec.canvas && rec.canvas.context.getConfiguration();
			if (config && !(config.usage & GPUTextureUsage.TEXTURE_BINDING)) rec.canvas.context.configure({ ...config, usage: config.usage | GPUTextureUsage.TEXTURE_BINDING });
		},
		// Called as the game records take frame `index`; its picture is the canvas texture the engine acquires next, once
		// the submit after that acquisition has drawn into it. A frame that drew nothing repeats the previous picture.
		frame(index) {
			const s = state;
			if (!s) return;
			if (!Number.isSafeInteger(index) || index < 0 || index <= (s.lastIndex ?? -1)) throw new RangeError('frame indices must increase');
			s.lastIndex = index;
			if (s.pending !== null) capture(s, s.pending, null);
			s.pending = index;
		},
		drawn(canvas) {
			const s = state;
			if (!s || s.pending === null) return;
			capture(s, s.pending, canvas);
			s.pending = null;
		},
		// Resolves to the Annex B stream once every frame is encoded, or null when none was.
		async end() {
			const s = state;
			if (s && s.pending !== null) capture(s, s.pending, null);
			state = null;
			if (!s) return null;
			await s.chain.catch((error) => { s.failed = true; rec.note('video readback failed: ' + error); });
			if (s.encoder && !s.failed) await s.encoder.flush().catch((error) => rec.note('video flush failed: ' + error));
			if (s.encoder && s.encoder.state !== 'closed') s.encoder.close();
			if (s.last) s.last.close();
			for (const slot of s.ring) { slot.buffer.destroy(); slot.target.destroy(); }
			if (!s.chunks.length) return null;
			const output = new Map(s.outputTimes.sort((a, b) => a - b).map((time, i) => [time, i]));
			for (const map of s.mapping) if (map.video_frame !== null) { const written = output.get(s.acceptedTimes[map.video_frame]); map.video_frame = written ?? null; if (written === undefined) map.status = 'encoder_failed'; }
			const out = new Uint8Array(s.chunks.reduce((n, c) => n + c.length, 0));
			let at = 0;
			for (const c of s.chunks) { out.set(c, at); at += c.length; }
			return { bytes: out, mapping: s.mapping, width: s.width, height: s.height, fps: 60, codec: 'avc1.64002A', hardware: 'browser-selected' };
		},
	};
} };
