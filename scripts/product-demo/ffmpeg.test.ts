import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  buildExecutableCandidates,
  buildFinalVideoArgs,
  buildLoudnormAnalysisArgs,
  buildLoudnormRenderArgs,
  buildProbeArgs,
  discoverMediaBinaries,
  parseLoudnormMeasurement,
  parseMediaProbe,
  probeMedia,
  runCommand,
  type CommandResult,
} from './ffmpeg.ts';
import {
  AUDIO_SAMPLE_RATE,
  DEFAULT_SOUND_EFFECTS,
  buildAudioMixArgs,
  buildBackgroundMusicArgs,
  buildSayCommand,
  buildSayVoiceListCommand,
  buildSoundEffectsArgs,
  buildVoiceover,
  buildVoiceoverConversionArgs,
  buildVoiceoverMixArgs,
  hasTingtingVoice,
  mixDemoAudio,
} from './audio.ts';

const ok = (stdout = '', stderr = ''): CommandResult => ({
  stdout,
  stderr,
  exitCode: 0,
});

function valueAfter(args: readonly string[], flag: string): string | undefined {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

test('最终视频参数固定为 30fps、2550 帧、H.264/AAC/yuv420p 和 faststart', () => {
  const videoPath = '/tmp/work-review-demo/input clip;$(touch nope).webm';
  const audioPath = '/tmp/work-review-demo/mix audio.wav';
  const outputPath = '/tmp/work-review-demo/final landscape.mp4';
  const args = buildFinalVideoArgs({
    videoPath,
    audioPath,
    outputPath,
    width: 1920,
    height: 1080,
  });

  assert.equal(valueAfter(args, '-r'), '30');
  assert.equal(valueAfter(args, '-frames:v'), '2550');
  assert.equal(valueAfter(args, '-c:v'), 'libx264');
  assert.equal(valueAfter(args, '-pix_fmt'), 'yuv420p');
  assert.equal(valueAfter(args, '-c:a'), 'aac');
  assert.equal(valueAfter(args, '-ar'), '48000');
  assert.equal(valueAfter(args, '-ac'), '2');
  assert.equal(valueAfter(args, '-t'), '85.000');
  assert.equal(valueAfter(args, '-movflags'), '+faststart');
  assert.match(valueAfter(args, '-vf') ?? '', /fps=30/);
  assert.match(valueAfter(args, '-vf') ?? '', /scale=1920:1080/);
  assert.ok(args.includes(videoPath));
  assert.ok(args.includes(audioPath));
  assert.equal(args.at(-1), outputPath);
  assert.ok(!args.some((arg) => arg.includes(`ffmpeg ${videoPath}`)));
});

test('响度规范化参数严格执行固定两遍 loudnorm', () => {
  const analysis = buildLoudnormAnalysisArgs('/tmp/work-review-demo/unmastered.wav');
  const measurement = {
    inputI: -22.4,
    inputTp: -3.2,
    inputLra: 4.1,
    inputThresh: -32.7,
    targetOffset: 0.3,
  };
  const render = buildLoudnormRenderArgs(
    '/tmp/work-review-demo/unmastered.wav',
    '/tmp/work-review-demo/mix.wav',
    measurement,
  );

  assert.match(valueAfter(analysis, '-af') ?? '', /loudnorm=I=-14:TP=-1:LRA=7/);
  assert.match(valueAfter(analysis, '-af') ?? '', /print_format=json/);
  assert.equal(valueAfter(analysis, '-f'), 'null');

  const renderFilter = valueAfter(render, '-af') ?? '';
  assert.match(renderFilter, /^loudnorm=I=-14:TP=-1:LRA=7:/);
  assert.match(renderFilter, /measured_I=-22\.4/);
  assert.match(renderFilter, /measured_TP=-3\.2/);
  assert.match(renderFilter, /measured_LRA=4\.1/);
  assert.match(renderFilter, /measured_thresh=-32\.7/);
  assert.match(renderFilter, /offset=0\.3/);
  assert.match(renderFilter, /linear=true/);
  assert.equal(valueAfter(render, '-ar'), '48000');
  assert.equal(valueAfter(render, '-ac'), '2');
  assert.equal(valueAfter(render, '-t'), '85.000');
});

test('解析 loudnorm 首遍 JSON，拒绝缺失或非数值字段', () => {
  const stderr = `frame=123\n[Parsed_loudnorm_0]\n{
    "input_i" : "-22.40",
    "input_tp" : "-3.20",
    "input_lra" : "4.10",
    "input_thresh" : "-32.70",
    "target_offset" : "0.30"
  }\n`;

  assert.deepEqual(parseLoudnormMeasurement(stderr), {
    inputI: -22.4,
    inputTp: -3.2,
    inputLra: 4.1,
    inputThresh: -32.7,
    targetOffset: 0.3,
  });
  assert.throws(() => parseLoudnormMeasurement('{"input_i":"-22"}'), /loudnorm.*字段/);
});

test('ffprobe 参数请求 JSON、流信息、格式信息和精确帧计数', () => {
  const mediaPath = '/tmp/work-review-demo/video with spaces.mp4';
  const args = buildProbeArgs(mediaPath);

  assert.deepEqual(args.slice(0, 2), ['-v', 'error']);
  assert.ok(args.includes('-show_streams'));
  assert.ok(args.includes('-show_format'));
  assert.ok(args.includes('-count_frames'));
  assert.equal(valueAfter(args, '-print_format'), 'json');
  assert.equal(args.at(-1), mediaPath);
});

test('媒体探测结果解析时长、帧率、帧数、编解码器和画面规格', () => {
  const result = parseMediaProbe(JSON.stringify({
    format: { duration: '85.000000', format_name: 'mov,mp4,m4a,3gp,3g2,mj2' },
    streams: [
      {
        index: 0,
        codec_type: 'video',
        codec_name: 'h264',
        pix_fmt: 'yuv420p',
        width: 1920,
        height: 1080,
        r_frame_rate: '30/1',
        avg_frame_rate: '30/1',
        nb_frames: '2550',
        nb_read_frames: '2550',
        duration: '85.000000',
      },
      {
        index: 1,
        codec_type: 'audio',
        codec_name: 'aac',
        sample_rate: '48000',
        channels: 2,
        channel_layout: 'stereo',
        duration: '85.000000',
      },
    ],
  }));

  assert.equal(result.durationSeconds, 85);
  assert.equal(result.formatName, 'mov,mp4,m4a,3gp,3g2,mj2');
  assert.deepEqual(result.video, {
    codec: 'h264',
    pixelFormat: 'yuv420p',
    width: 1920,
    height: 1080,
    frameRate: 30,
    averageFrameRate: 30,
    frameCount: 2550,
    durationSeconds: 85,
  });
  assert.deepEqual(result.audio, {
    codec: 'aac',
    sampleRate: 48000,
    channels: 2,
    channelLayout: 'stereo',
    durationSeconds: 85,
  });
});

test('媒体探测支持读取帧数回退并拒绝无效 JSON 或无媒体流', () => {
  const fallback = parseMediaProbe(JSON.stringify({
    format: { duration: '1.5' },
    streams: [{
      codec_type: 'video',
      codec_name: 'h264',
      width: 1080,
      height: 1920,
      avg_frame_rate: '30000/1001',
      nb_frames: 'N/A',
      nb_read_frames: '45',
    }],
  }));

  assert.equal(fallback.video?.frameCount, 45);
  assert.ok(Math.abs((fallback.video?.averageFrameRate ?? 0) - 29.97002997) < 1e-6);
  assert.throws(() => parseMediaProbe('not-json'), /ffprobe JSON/);
  assert.throws(() => parseMediaProbe('null'), /ffprobe JSON.*对象/);
  const unknownDuration = parseMediaProbe(JSON.stringify({
    format: {},
    streams: [{ codec_type: 'audio', codec_name: 'aac' }],
  }));
  assert.equal(unknownDuration.durationSeconds, null);

  assert.throws(
    () => parseMediaProbe(JSON.stringify({ format: { duration: '85' }, streams: [] })),
    /未发现音视频流/,
  );
});

test('probeMedia 通过参数数组调用指定 ffprobe 并解析 stdout', async () => {
  const calls: Array<{ binary: string; args: readonly string[] }> = [];
  const result = await probeMedia('/tmp/work-review-demo/sample.mp4', {
    ffprobePath: '/opt/tools/ffprobe',
    runner: async (binary, args) => {
      calls.push({ binary, args });
      return ok(JSON.stringify({
        format: { duration: '85.0' },
        streams: [{ codec_type: 'audio', codec_name: 'aac', sample_rate: '48000', channels: 2 }],
      }));
    },
  });

  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.binary, '/opt/tools/ffprobe');
  assert.equal(calls[0]?.args.at(-1), '/tmp/work-review-demo/sample.mp4');
  assert.equal(result.audio?.codec, 'aac');
});

test('可执行文件候选优先环境覆盖，并从 PATH 与 macOS 常见目录发现工具', async () => {
  const env = {
    PATH: '/custom/bin:/usr/bin',
    FFMPEG_PATH: '/private/tools/ffmpeg custom',
    FFPROBE_PATH: '/private/tools/ffprobe custom',
  };
  const ffmpegCandidates = buildExecutableCandidates('ffmpeg', { env, platform: 'darwin' });

  assert.equal(ffmpegCandidates[0], '/private/tools/ffmpeg custom');
  assert.ok(ffmpegCandidates.includes('/custom/bin/ffmpeg'));
  assert.ok(ffmpegCandidates.includes('/opt/homebrew/bin/ffmpeg'));

  const executable = new Set(['/private/tools/ffmpeg custom', '/custom/bin/ffprobe']);
  const verified: string[] = [];
  const binaries = await discoverMediaBinaries({
    env,
    platform: 'darwin',
    isExecutable: async (candidate) => executable.has(candidate),
    runner: async (binary, args) => {
      verified.push(`${binary}\0${args.join('\0')}`);
      return ok(`${binary} version 8.0`);
    },
  });

  assert.deepEqual(binaries, {
    ffmpeg: '/private/tools/ffmpeg custom',
    ffprobe: '/custom/bin/ffprobe',
  });
  assert.deepEqual(verified, [
    '/private/tools/ffmpeg custom\0-version',
    '/custom/bin/ffprobe\0-version',
  ]);
});

test('找不到 ffmpeg 或 ffprobe 时给出明确错误', async () => {
  await assert.rejects(
    discoverMediaBinaries({
      env: { PATH: '/missing' },
      platform: 'darwin',
      isExecutable: async () => false,
      runner: async () => ok(),
    }),
    /未找到可用的 ffmpeg/,
  );
});

test('runCommand 不启用 shell，危险字符和空格保持单个参数', async () => {
  const dangerous = '/tmp/work-review-demo/clip; echo SHOULD_NOT_RUN $(uname).webm';
  const result = await runCommand(process.execPath, [
    '-e',
    'process.stdout.write(JSON.stringify(process.argv.slice(1)))',
    dangerous,
    '第二个参数',
  ]);

  assert.deepEqual(JSON.parse(result.stdout), [dangerous, '第二个参数']);
});

test('runCommand 失败信息包含退出码和截断后的 stderr', async () => {
  await assert.rejects(
    runCommand(
      process.execPath,
      ['-e', 'process.stderr.write("PREFIX-" + "x".repeat(400) + "-FINAL ERROR"); process.exit(7)'],
      { maxCaptureBytes: 64 },
    ),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /退出码 7/);
      assert.match(error.message, /已截断/);
      assert.match(error.message, /FINAL ERROR/);
      assert.doesNotMatch(error.message, /PREFIX/);
      assert.ok(error.message.length < 300);
      return true;
    },
  );
});

test('Tingting 旁白命令保持文本和路径为独立参数，不经过 shell', () => {
  const list = buildSayVoiceListCommand();
  assert.deepEqual(list, { binary: '/usr/bin/say', args: ['-v', '?'] });
  assert.equal(hasTingtingVoice('Tingting            zh_CN    # 您好'), true);
  assert.equal(hasTingtingVoice('Meijia              zh_TW    # 您好'), false);

  const text = '日报完成；$(touch /tmp/nope) && echo 不执行';
  const outputPath = '/tmp/work-review-demo/voice cue 01.aiff';
  const command = buildSayCommand(text, outputPath);

  assert.equal(command.binary, '/usr/bin/say');
  assert.deepEqual(command.args, ['-v', 'Tingting', '-o', outputPath, text]);
});

test('旁白 AIFF 转 WAV 固定 48kHz 双声道，时间线混合固定 85 秒', () => {
  const conversion = buildVoiceoverConversionArgs('cue.aiff', 'cue.wav');
  assert.equal(valueAfter(conversion, '-ar'), String(AUDIO_SAMPLE_RATE));
  assert.equal(valueAfter(conversion, '-ac'), '2');
  assert.equal(valueAfter(conversion, '-c:a'), 'pcm_s24le');
  assert.equal(conversion.at(-1), 'cue.wav');

  const mix = buildVoiceoverMixArgs([
    { path: 'first.wav', startSeconds: 0.35 },
    { path: 'second.wav', startSeconds: 7.35 },
  ], 'voiceover.wav');
  const filter = valueAfter(mix, '-filter_complex') ?? '';
  assert.match(filter, /adelay=350\|350/);
  assert.match(filter, /adelay=7350\|7350/);
  assert.match(filter, /amix=inputs=2:normalize=0/);
  assert.match(filter, /atrim=0:85/);
  assert.match(filter, /apad=whole_dur=85/);
  assert.equal(valueAfter(mix, '-ar'), '48000');
  assert.equal(valueAfter(mix, '-ac'), '2');
});

test('buildVoiceover 可注入命令执行器，单句超时会报告实际时长而不强制加速', async () => {
  const outputDir = await mkdtemp(join(tmpdir(), 'work-review-voiceover-'));
  const calls: Array<{ binary: string; args: readonly string[] }> = [];

  await assert.rejects(
    buildVoiceover(
      [{ id: 'too-long', start: 78.35, end: 84.65, zh: '最后一句', en: 'Last line' }],
      {
        outputDir,
        ffmpegPath: '/mock/ffmpeg',
        runner: async (binary, args) => {
          calls.push({ binary, args });
          if (binary === '/usr/bin/say' && args[1] === '?') {
            return ok('Tingting            zh_CN    # 您好');
          }
          return ok();
        },
        probe: async () => ({
          durationSeconds: 5.2,
          formatName: 'wav',
          video: null,
          audio: {
            codec: 'pcm_s24le',
            sampleRate: 48000,
            channels: 2,
            channelLayout: 'stereo',
            durationSeconds: 5.2,
          },
        }),
      },
    ),
    /too-long.*实际 5\.200 秒.*83\.000 秒前/,
  );

  assert.equal(calls[0]?.binary, '/usr/bin/say');
  assert.deepEqual(calls[0]?.args, ['-v', '?']);
  assert.ok(calls.some(({ binary, args }) => binary === '/usr/bin/say' && args[0] === '-v'
    && args[1] === 'Tingting' && args.includes('最后一句')));
  assert.ok(calls.some(({ binary }) => binary === '/mock/ffmpeg'));
});

test('程序化背景音乐使用正弦波、低通、淡入淡出并自然收束到 85 秒', () => {
  const args = buildBackgroundMusicArgs('/tmp/work-review-demo/background.wav');
  const joined = args.join(' ');

  assert.match(joined, /sine=frequency=/);
  assert.match(joined, /sample_rate=48000/);
  assert.match(joined, /duration=85/);
  const filter = valueAfter(args, '-filter_complex') ?? '';
  assert.match(filter, /amix=inputs=3:normalize=0:duration=longest,lowpass=/);
  assert.match(filter, /lowpass=/);
  assert.match(filter, /afade=t=in/);
  assert.match(valueAfter(args, '-filter_complex') ?? '', /afade=t=out:st=80:d=5/);
  assert.match(valueAfter(args, '-filter_complex') ?? '', /atrim=0:85/);
  assert.equal(valueAfter(args, '-ar'), '48000');
  assert.equal(valueAfter(args, '-ac'), '2');
});

test('点击、切换、完成和片尾音效由短包络正弦波程序化生成', () => {
  const args = buildSoundEffectsArgs(DEFAULT_SOUND_EFFECTS, '/tmp/work-review-demo/effects.wav');
  const filter = valueAfter(args, '-filter_complex') ?? '';

  assert.deepEqual(DEFAULT_SOUND_EFFECTS.map(({ kind }) => kind), [
    'click',
    'switch',
    'complete',
    'outro',
  ]);
  assert.equal(args.filter((arg) => arg === '-f').length, DEFAULT_SOUND_EFFECTS.length);
  assert.match(args.join(' '), /sine=frequency=/);
  assert.match(filter, /adelay=/);
  assert.match(filter, /afade=t=out/);
  assert.match(filter, /amix=inputs=4:normalize=0/);
  assert.match(filter, /atrim=0:85/);
});

test('最终混音在旁白区间把音乐降低 10dB，并限制时长与峰值', () => {
  const args = buildAudioMixArgs({
    voiceoverPath: 'voiceover.wav',
    musicPath: 'music.wav',
    effectsPath: 'effects.wav',
    outputPath: 'unmastered.wav',
    voiceoverCues: [
      { id: 'one', start: 0.35, end: 6.65, zh: '一', en: 'one' },
      { id: 'two', start: 7.35, end: 18.65, zh: '二', en: 'two' },
    ],
  });
  const filter = valueAfter(args, '-filter_complex') ?? '';

  assert.match(filter, /volume=0\.316228/);
  assert.match(filter, /between\(t,0\.350,6\.650\)/);
  assert.match(filter, /between\(t,7\.350,18\.650\)/);
  assert.match(filter, /amix=inputs=3:normalize=0/);
  assert.match(filter, /alimiter=limit=0\.891251/);
  assert.match(filter, /atrim=0:85/);
  assert.match(filter, /apad=whole_dur=85/);
});

test('mixDemoAudio 依次执行原始混音、首遍分析和第二遍渲染', async () => {
  const calls: Array<{ binary: string; args: readonly string[] }> = [];
  const outputPath = '/tmp/work-review-demo/mix.wav';
  const result = await mixDemoAudio({
    voiceoverPath: '/tmp/work-review-demo/voiceover.wav',
    musicPath: '/tmp/work-review-demo/music.wav',
    effectsPath: '/tmp/work-review-demo/effects.wav',
    outputPath,
    voiceoverCues: [{ id: 'one', start: 0.35, end: 6.65, zh: '一', en: 'one' }],
    ffmpegPath: '/mock/ffmpeg',
    runner: async (binary, args) => {
      calls.push({ binary, args });
      if (args.includes('null')) {
        return ok('', `{
          "input_i":"-20.0",
          "input_tp":"-3.0",
          "input_lra":"3.0",
          "input_thresh":"-30.0",
          "target_offset":"0.2"
        }`);
      }
      return ok();
    },
  });

  assert.equal(result, outputPath);
  assert.equal(calls.length, 3);
  assert.ok(calls.every(({ binary }) => binary === '/mock/ffmpeg'));
  assert.match(valueAfter(calls[1]?.args ?? [], '-af') ?? '', /print_format=json/);
  assert.match(valueAfter(calls[2]?.args ?? [], '-af') ?? '', /measured_I=-20/);
  assert.equal(calls[2]?.args.at(-1), outputPath);
});
