<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { cubicOut } from 'svelte/easing';
  import { tweened } from 'svelte/motion';
  import {
    getAvatarPresetDefinition,
    normalizeAvatarPresetId,
    type AvatarMouseMotionModel,
    type AvatarPreviewInputActivity,
  } from './avatarPresetRegistry.ts';
  import {
    getAvatarIdleMotionMeta,
    getAvatarModeMeta,
    type AvatarMode,
  } from './avatarStateMeta.ts';
  import { t } from '$lib/i18n/index.ts';

  interface Point {
    x: number;
    y: number;
  }

  interface AvatarCanvasState {
    mode: AvatarMode;
    appName: string;
    contextLabel: string;
    hint: string;
    isIdle: boolean;
    isGeneratingReport: boolean;
    avatarOpacity: number;
    avatarPreset: string;
  }

  interface AvatarCanvasEvents {
    avatarpointerdown: { originalEvent: MouseEvent };
    avataractivate: { originalEvent: MouseEvent };
  }

  const dispatch = createEventDispatcher<AvatarCanvasEvents>();
  const SCENE_WIDTH = 612;
  const SCENE_HEIGHT = 354;
  const DEVICE_WIDTH = 104;
  const DEVICE_HEIGHT = 103;
  const STANDARD_DEVICE_OFFSET_X = 10;
  const STANDARD_DEVICE_OFFSET_Y = -10;
  const STANDARD_HAND_DX = -38;
  const STANDARD_HAND_DY = -50;
  const clipIdBase = `avatar-scene-${Math.random().toString(36).slice(2, 10)}`;
  const STANDARD_SCENE_MODES = new Set<AvatarMode>([
    'idle',
    'working',
    'reading',
    'meeting',
    'music',
    'video',
    'generating',
    'slacking',
  ]);
  const cursorRatioXTween = tweened(0.5, { duration: 70, easing: cubicOut });
  const cursorRatioYTween = tweened(0.5, { duration: 70, easing: cubicOut });
  let lastCursorTargetX = 0.5;
  let lastCursorTargetY = 0.5;

  const ARM_CONTROL_COUNT = 19;
  const ARM_SAMPLE_ITER = 25;
  const ARM_BERNSTEIN_TABLE = (() => {
    const degree = ARM_CONTROL_COUNT - 1;
    const binom = new Float64Array(ARM_CONTROL_COUNT);
    binom[0] = 1;
    for (let k = 1; k <= degree; k += 1) {
      binom[k] = (binom[k - 1] * (degree - k + 1)) / k;
    }
    const table = new Float64Array((ARM_SAMPLE_ITER - 1) * ARM_CONTROL_COUNT);
    for (let index = 1; index < ARM_SAMPLE_ITER; index += 1) {
      const ratio = index / ARM_SAMPLE_ITER;
      const inv = 1 - ratio;
      const rowOffset = (index - 1) * ARM_CONTROL_COUNT;
      for (let k = 0; k < ARM_CONTROL_COUNT; k += 1) {
        table[rowOffset + k] = binom[k] * Math.pow(inv, degree - k) * Math.pow(ratio, k);
      }
    }
    return table;
  })();
  const standardArmControlCoords = new Float64Array(ARM_CONTROL_COUNT * 2);

  function clamp01(value: number): number {
    return Math.min(1, Math.max(0, value));
  }

  function computeStandardMouseGeometry(cursorRatioX = 0.5, cursorRatioY = 0.5) {
    const fx = clamp01(cursorRatioX);
    const fy = clamp01(cursorRatioY);
    const x = -97 * fx + 44 * fy + 184;
    const y = -76 * fx - 40 * fy + 324;
    const dx = STANDARD_HAND_DX;
    const dy = STANDARD_HAND_DY;
    const pss = standardArmControlCoords;
    const oof = 6;

    let writeIndex = 0;
    pss[writeIndex++] = 211;
    pss[writeIndex++] = 159;

    let dist = Math.hypot(211 - x, 159 - y);
    const centreleft0 = 211 - (0.7237 * dist) / 2;
    const centreleft1 = 159 + (0.69 * dist) / 2;

    for (let index = 1; index < oof; index += 1) {
      const t = index / oof;
      const inv = 1 - t;
      const inv2 = inv * inv;
      const mid = 2 * inv * t;
      const t2 = t * t;
      pss[writeIndex++] = inv2 * 211 + mid * centreleft0 + t2 * x;
      pss[writeIndex++] = inv2 * 159 + mid * centreleft1 + t2 * y;
    }
    pss[writeIndex++] = x;
    pss[writeIndex++] = y;

    let a = y - centreleft1;
    let b = centreleft0 - x;
    let length = Math.hypot(a, b) || 1;
    a = x + (a / length) * 60;
    b = y + (b / length) * 60;

    const a1 = 258;
    const a2 = 228;
    dist = Math.hypot(a1 - a, a2 - b);
    const centreright0 = a1 - (0.6 * dist) / 2;
    const centreright1 = a2 + (0.8 * dist) / 2;
    const push = 20;

    let s = x - centreleft0;
    let t = y - centreleft1;
    length = Math.hypot(s, t) || 1;
    s *= push / length;
    t *= push / length;

    let s2 = a - centreright0;
    let t2 = b - centreright1;
    length = Math.hypot(s2, t2) || 1;
    s2 *= push / length;
    t2 *= push / length;

    const c1x = x + s;
    const c1y = y + t;
    const c2x = a + s2;
    const c2y = b + t2;
    for (let index = 1; index < oof; index += 1) {
      const u = index / oof;
      const inv = 1 - u;
      const inv2 = inv * inv;
      const u2 = u * u;
      const w0 = inv2 * inv;
      const w1 = 3 * inv2 * u;
      const w2 = 3 * inv * u2;
      const w3 = u2 * u;
      pss[writeIndex++] = w0 * x + w1 * c1x + w2 * c2x + w3 * a;
      pss[writeIndex++] = w0 * y + w1 * c1y + w2 * c2y + w3 * b;
    }
    pss[writeIndex++] = a;
    pss[writeIndex++] = b;

    for (let index = oof - 1; index > 0; index -= 1) {
      const u = index / oof;
      const inv = 1 - u;
      const inv2 = inv * inv;
      const mid = 2 * inv * u;
      const u2 = u * u;
      pss[writeIndex++] = inv2 * a1 + mid * centreright0 + u2 * a;
      pss[writeIndex++] = inv2 * a2 + mid * centreright1 + u2 * b;
    }
    pss[writeIndex++] = a1;
    pss[writeIndex++] = a2;

    const mouseX = (a + x) / 2 - 67 + dx + STANDARD_DEVICE_OFFSET_X;
    const mouseY = (b + y) / 2 - 29 + dy + STANDARD_DEVICE_OFFSET_Y;
    const armPoints: Point[] = new Array(ARM_SAMPLE_ITER + 1);
    armPoints[0] = { x: pss[0] + dx, y: pss[1] + dy };

    for (let index = 1; index < ARM_SAMPLE_ITER; index += 1) {
      const rowOffset = (index - 1) * ARM_CONTROL_COUNT;
      let px = 0;
      let py = 0;
      for (let k = 0; k < ARM_CONTROL_COUNT; k += 1) {
        const weight = ARM_BERNSTEIN_TABLE[rowOffset + k];
        px += weight * pss[k * 2];
        py += weight * pss[k * 2 + 1];
      }
      armPoints[index] = { x: px + dx, y: py + dy };
    }
    armPoints[ARM_SAMPLE_ITER] = {
      x: pss[writeIndex - 2] + dx,
      y: pss[writeIndex - 1] + dy,
    };

    return { armPoints, mouseX, mouseY };
  }

  function getMousePointerIndicatorTone(mouseGroup: string) {
    switch (mouseGroup) {
      case 'mouse-left':
        return {
          pawFill: 'rgba(255, 255, 255, 0.96)',
          pawStroke: 'rgba(15, 23, 42, 0.86)',
          padFill: 'rgba(244, 114, 182, 0.72)',
        };
      case 'mouse-right':
        return {
          pawFill: 'rgba(255, 255, 255, 0.96)',
          pawStroke: 'rgba(15, 23, 42, 0.86)',
          padFill: 'rgba(250, 204, 21, 0.72)',
        };
      case 'mouse-side':
        return {
          pawFill: 'rgba(255, 255, 255, 0.96)',
          pawStroke: 'rgba(15, 23, 42, 0.86)',
          padFill: 'rgba(148, 163, 184, 0.72)',
        };
      default:
        return {
          pawFill: 'rgba(255, 255, 255, 0.96)',
          pawStroke: 'rgba(15, 23, 42, 0.86)',
          padFill: 'rgba(34, 211, 238, 0.72)',
        };
    }
  }

  function computeStaticSceneMouseGeometry(
    mouseMotionModel: AvatarMouseMotionModel | undefined,
    cursorRatioX = 0.5,
    cursorRatioY = 0.5,
  ) {
    if (!mouseMotionModel?.bounds) {
      return null;
    }

    const ratioX = clamp01(cursorRatioX);
    const ratioY = clamp01(cursorRatioY);
    const { anchorX, anchorY, bounds, bend = 0.24 } = mouseMotionModel;
    const targetX = bounds.x + bounds.width * ratioX;
    const targetY = bounds.y + bounds.height * ratioY;
    const dx = targetX - anchorX;
    const dy = targetY - anchorY;
    const distance = Math.hypot(dx, dy) || 1;
    const normalX = -dy / distance;
    const normalY = dx / distance;
    const controlX = anchorX + dx * 0.5 + normalX * distance * bend;
    const controlY = anchorY + dy * 0.5 + normalY * distance * bend;
    const samples = 18;
    const points: Point[] = [];

    for (let index = 0; index <= samples; index += 1) {
      const t = index / samples;
      const inv = 1 - t;
      points.push({
        x: inv * inv * anchorX + 2 * inv * t * controlX + t * t * targetX,
        y: inv * inv * anchorY + 2 * inv * t * controlY + t * t * targetY,
      });
    }

    const pawAngle = (Math.atan2(dy, dx) * 180) / Math.PI;

    return {
      armPoints: points.map((point) => `${point.x},${point.y}`).join(' '),
      pawX: targetX,
      pawY: targetY,
      pawAngle,
    };
  }

  function deriveKeyboardGroupFromVisualKey(baseGroup: string, visualKey: string): string {
    if (baseGroup && baseGroup !== 'idle') {
      return baseGroup;
    }

    switch (visualKey) {
      case 'Num1':
        return 'digit-1';
      case 'Num2':
        return 'digit-2';
      case 'Num3':
        return 'digit-3';
      case 'Num4':
        return 'digit-4';
      case 'Num5':
        return 'digit-5';
      case 'Num6':
        return 'digit-6';
      case 'Num7':
      case 'Num8':
      case 'Num9':
      case 'Num0':
        return 'digit-7';
      case 'KeyQ':
        return 'key-q';
      case 'KeyW':
        return 'key-w';
      case 'KeyE':
        return 'key-e';
      case 'KeyR':
      case 'KeyT':
      case 'KeyY':
      case 'KeyU':
      case 'KeyI':
      case 'KeyO':
      case 'KeyP':
        return 'key-r';
      case 'KeyA':
        return 'key-a';
      case 'KeyS':
        return 'key-s';
      case 'KeyD':
      case 'KeyF':
      case 'KeyG':
      case 'KeyH':
      case 'KeyJ':
      case 'KeyK':
      case 'KeyL':
        return 'key-d';
      case 'KeyZ':
      case 'KeyX':
      case 'KeyC':
        return 'key-a';
      case 'KeyV':
      case 'KeyB':
        return 'key-s';
      case 'KeyN':
      case 'KeyM':
      case 'Slash':
        return 'key-d';
      case 'Space':
      case 'Return':
      case 'Tab':
      case 'Backspace':
      case 'Delete':
        return 'space';
      case 'BackQuote':
      case 'Escape':
      case 'CapsLock':
      case 'Shift':
      case 'ShiftLeft':
      case 'ShiftRight':
      case 'Control':
      case 'ControlLeft':
      case 'ControlRight':
      case 'Alt':
      case 'AltGr':
      case 'Meta':
      case 'Fn':
        return 'key-q';
      default:
        return 'idle';
    }
  }

  export let state: AvatarCanvasState = {
    mode: 'idle',
    appName: 'Work Review',
    contextLabel: '待命中',
    hint: '',
    isIdle: true,
    isGeneratingReport: false,
    avatarOpacity: 0.82,
    avatarPreset: 'original-standard',
  };
  export let inputActivity: AvatarPreviewInputActivity = {
    keyboardActive: false,
    mouseActive: false,
    keyboardGroup: 'idle',
    keyboardVisualKey: '',
    mouseGroup: 'idle',
    cursorRatioX: 0.5,
    cursorRatioY: 0.5,
    lastKeyboardInputAtMs: 0,
    lastMouseInputAtMs: 0,
  };
  export let transitionClass = '';
  export let motionBeat = 0;

  $: modeMeta = getAvatarModeMeta(state.mode, state.contextLabel);
  $: idleMotionMeta = getAvatarIdleMotionMeta(state.mode, state.contextLabel, motionBeat);
  $: shellClass = ['avatar-shell', modeMeta.shellClass, idleMotionMeta.shellClass, transitionClass]
    .filter(Boolean)
    .join(' ');
  $: showKeyboardLayers = STANDARD_SCENE_MODES.has(state.mode);
  $: showMouseLayers = STANDARD_SCENE_MODES.has(state.mode);
  $: keyboardActive = showKeyboardLayers && !!inputActivity.keyboardActive;
  $: mouseActive = showMouseLayers && !!inputActivity.mouseActive;
  $: keyboardVisualKey = inputActivity.keyboardVisualKey || '';
  $: keyboardGroup = deriveKeyboardGroupFromVisualKey(
    inputActivity.keyboardGroup || 'idle',
    keyboardVisualKey
  );
  $: mouseGroup = inputActivity.mouseGroup || 'idle';
  $: preset = getAvatarPresetDefinition(normalizeAvatarPresetId(state.avatarPreset));
  $: renderMode = preset.renderMode ?? 'standard';
  $: sceneInteractionLayout = preset.interactionLayout ?? null;
  $: cursorTweenDuration = mouseActive ? 44 : 110;
  // 仅当目标光标比例真正变化时重设补间，避免敲键盘或 mouseActive 回落时反复重启已就位的补间动画。
  $: {
    const nextTargetX = inputActivity.cursorRatioX ?? 0.5;
    if (nextTargetX !== lastCursorTargetX) {
      lastCursorTargetX = nextTargetX;
      cursorRatioXTween.set(nextTargetX, {
        duration: cursorTweenDuration,
        easing: cubicOut,
      });
    }
  }
  $: {
    const nextTargetY = inputActivity.cursorRatioY ?? 0.5;
    if (nextTargetY !== lastCursorTargetY) {
      lastCursorTargetY = nextTargetY;
      cursorRatioYTween.set(nextTargetY, {
        duration: cursorTweenDuration,
        easing: cubicOut,
      });
    }
  }
  $: cursorRatioX = $cursorRatioXTween;
  $: cursorRatioY = $cursorRatioYTween;
  $: sceneSrc = preset.sceneSrc;
  $: contentTransform = preset.contentTransform ?? '';
  $: staticCoverSrc = preset.staticCoverSrc ?? null;
  $: sceneAlt = t('avatar.sceneAlt', { name: state.appName || 'Work Review' });
  $: frameIndex = Math.floor(motionBeat / 2);
  $: useSourceKeyboardMode = renderMode === 'source-keyboard';
  $: standardHandSrc = keyboardActive
    ? preset.handFrames[keyboardGroup] ?? preset.idleHand
    : preset.idleHand;
  $: keyOverlaySrc = keyboardActive
    ? preset.keyboardFrames[keyboardGroup] ?? null
    : null;
  $: keyboardVisualSrc = keyboardActive
    ? preset.keyboardVisualLayers?.[keyboardVisualKey] ?? null
    : null;
  $: keyboardVisualClip = preset.keyboardVisualClip ?? null;
  $: keyboardVisualClipId = keyboardVisualClip ? `${clipIdBase}-keyboard-visual` : '';
  $: keyboardVisualClipUrl = keyboardVisualClipId ? `url(#${keyboardVisualClipId})` : '';
  $: keyboardHotspotsAboveCover = !!preset.keyboardHotspotsAboveCover;
  $: leftHandSrc = keyboardActive
    ? preset.leftHandFrames?.[keyboardVisualKey] ?? preset.idleLeftHand ?? null
    : preset.idleLeftHand ?? null;
  $: rightHandSrc = keyboardActive
    ? preset.rightHandFrames?.[keyboardVisualKey] ?? preset.idleRightHand ?? null
    : preset.idleRightHand ?? null;
  $: keyOverlayPose = keyboardActive ? frameIndex % 3 : 0;
  $: mouseDeviceBaseSrc = preset.mouseDeviceBase;
  $: mouseDeviceOverlaySrc = preset.mouseDeviceOverlays[mouseGroup] ?? null;
  $: mouseVisualSrc = mouseActive
    ? preset.mouseVisualLayers?.[mouseGroup] ?? null
    : null;
  $: mouseVisualClip = preset.mouseVisualClip ?? null;
  $: mouseVisualClipId = mouseVisualClip ? `${clipIdBase}-mouse-visual` : '';
  $: mouseVisualClipUrl = mouseVisualClipId ? `url(#${mouseVisualClipId})` : '';
  $: staticSceneMouseGeometry = mouseActive && preset.mouseMotionModel
    ? computeStaticSceneMouseGeometry(preset.mouseMotionModel, cursorRatioX, cursorRatioY)
    : null;
  $: mousePointerIndicatorTone = getMousePointerIndicatorTone(mouseGroup);
  $: mouseHotspotsAboveCover = !!preset.mouseHotspotsAboveCover;
  $: mouseGeometry = computeStandardMouseGeometry(cursorRatioX, cursorRatioY);
  $: mouseArmPoints = staticSceneMouseGeometry?.armPoints
    ?? mouseGeometry.armPoints.map((point) => `${point.x},${point.y}`).join(' ');
  $: mouseDeviceX = mouseGeometry.mouseX;
  $: mouseDeviceY = mouseGeometry.mouseY;
  $: keyboardHotspots = keyboardActive && sceneInteractionLayout
    ? [keyboardGroup].flatMap((groupName) => sceneInteractionLayout.keyboardHotspots?.[groupName] ?? [])
    : [];
  $: mouseHotspots = mouseActive && sceneInteractionLayout && !mouseVisualSrc && !preset.mouseMotionModel
    ? sceneInteractionLayout.mouseHotspots?.[mouseGroup] ?? []
    : [];
  $: preCoverKeyboardHotspots = keyboardHotspotsAboveCover ? [] : keyboardHotspots;
  $: postCoverKeyboardHotspots = keyboardHotspotsAboveCover ? keyboardHotspots : [];
  $: preCoverMouseHotspots = mouseHotspotsAboveCover ? [] : mouseHotspots;
  $: postCoverMouseHotspots = mouseHotspotsAboveCover ? mouseHotspots : [];
  $: keyboardOverlayStyle = `opacity:${preset.keyboardOverlayOpacity ?? 1};`;
  $: handStyle = `opacity:${preset.handOpacity ?? 1};`;
  $: mouseDeviceStyle = `opacity:${preset.mouseDeviceOpacity ?? 1};`;
  $: mouseOverlayStyle = `opacity:${preset.mouseOverlayOpacity ?? 1};`;
  $: mouseArmStyle = `opacity:${preset.mouseArmOpacity ?? 1};`;
  $: shellStyle = `--avatar-shell-opacity:${state.avatarOpacity ?? 0.82};`;

  function handlePointerDown(event: MouseEvent): void {
    dispatch('avatarpointerdown', { originalEvent: event });
  }

  function handleActivate(event: MouseEvent): void {
    dispatch('avataractivate', { originalEvent: event });
  }
</script>

<div class="relative flex h-full w-full items-end overflow-visible select-none">
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    class={shellClass}
    style={shellStyle}
    on:mousedown={handlePointerDown}
    on:dblclick={handleActivate}
  >
    <div class="scene-frame">
      <svg
        class="scene-svg scene-base"
        viewBox={`0 0 ${SCENE_WIDTH} ${SCENE_HEIGHT}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={sceneAlt}
      >
        <defs>
          {#if keyboardVisualClip}
            <clipPath id={keyboardVisualClipId}>
              {#if keyboardVisualClip.kind === 'rect'}
                <rect
                  x={keyboardVisualClip.x}
                  y={keyboardVisualClip.y}
                  width={keyboardVisualClip.width}
                  height={keyboardVisualClip.height}
                  rx={keyboardVisualClip.rx ?? 5}
                  transform={keyboardVisualClip.transform || undefined}
                />
              {:else if keyboardVisualClip.kind === 'ellipse'}
                <ellipse
                  cx={keyboardVisualClip.cx}
                  cy={keyboardVisualClip.cy}
                  rx={keyboardVisualClip.rx}
                  ry={keyboardVisualClip.ry}
                />
              {:else if keyboardVisualClip.kind === 'polygon'}
                <polygon
                  points={keyboardVisualClip.points}
                  stroke-linejoin="round"
                />
              {/if}
            </clipPath>
          {/if}

          {#if mouseVisualClip}
            <clipPath id={mouseVisualClipId}>
              {#if mouseVisualClip.kind === 'rect'}
                <rect
                  x={mouseVisualClip.x}
                  y={mouseVisualClip.y}
                  width={mouseVisualClip.width}
                  height={mouseVisualClip.height}
                  rx={mouseVisualClip.rx ?? 5}
                  transform={mouseVisualClip.transform || undefined}
                />
              {:else if mouseVisualClip.kind === 'ellipse'}
                <ellipse
                  cx={mouseVisualClip.cx}
                  cy={mouseVisualClip.cy}
                  rx={mouseVisualClip.rx}
                  ry={mouseVisualClip.ry}
                />
              {:else if mouseVisualClip.kind === 'polygon'}
                <polygon
                  points={mouseVisualClip.points}
                  stroke-linejoin="round"
                />
              {/if}
            </clipPath>
          {/if}
        </defs>

        <g transform={contentTransform}>
          <image href={sceneSrc} x="0" y="0" width={SCENE_WIDTH} height={SCENE_HEIGHT} />

          {#if preCoverKeyboardHotspots.length || preCoverMouseHotspots.length}
            <g class="scene-interaction-layer">
              {#each preCoverKeyboardHotspots as hotspot}
                {#if hotspot.kind === 'rect'}
                  <rect
                    class="scene-hotspot"
                    x={hotspot.x}
                    y={hotspot.y}
                    width={hotspot.width}
                    height={hotspot.height}
                    rx={hotspot.rx ?? 5}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    transform={hotspot.transform || undefined}
                  />
                {:else if hotspot.kind === 'ellipse'}
                  <ellipse
                    class="scene-hotspot"
                    cx={hotspot.cx}
                    cy={hotspot.cy}
                    rx={hotspot.rx}
                    ry={hotspot.ry}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                  />
                {:else if hotspot.kind === 'polygon'}
                  <polygon
                    class="scene-hotspot"
                    points={hotspot.points}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    stroke-linejoin="round"
                  />
                {/if}
              {/each}

              {#each preCoverMouseHotspots as hotspot}
                {#if hotspot.kind === 'rect'}
                  <rect
                    class="scene-hotspot scene-hotspot-mouse"
                    x={hotspot.x}
                    y={hotspot.y}
                    width={hotspot.width}
                    height={hotspot.height}
                    rx={hotspot.rx ?? 5}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    transform={hotspot.transform || undefined}
                  />
                {:else if hotspot.kind === 'ellipse'}
                  <ellipse
                    class="scene-hotspot scene-hotspot-mouse"
                    cx={hotspot.cx}
                    cy={hotspot.cy}
                    rx={hotspot.rx}
                    ry={hotspot.ry}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                  />
                {:else if hotspot.kind === 'polygon'}
                  <polygon
                    class="scene-hotspot scene-hotspot-mouse"
                    points={hotspot.points}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    stroke-linejoin="round"
                  />
                {/if}
              {/each}
            </g>
          {/if}

          {#if staticCoverSrc}
            <image
              href={staticCoverSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="static-cover-layer"
            />
          {/if}

          {#if postCoverKeyboardHotspots.length}
            <g
              class="scene-interaction-layer post-cover-keyboard-hotspot-layer"
              clip-path={keyboardVisualClipUrl || undefined}
            >
              {#each postCoverKeyboardHotspots as hotspot}
                {#if hotspot.kind === 'rect'}
                  <rect
                    class="scene-hotspot scene-hotspot-keyboard"
                    x={hotspot.x}
                    y={hotspot.y}
                    width={hotspot.width}
                    height={hotspot.height}
                    rx={hotspot.rx ?? 5}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    transform={hotspot.transform || undefined}
                  />
                {:else if hotspot.kind === 'ellipse'}
                  <ellipse
                    class="scene-hotspot scene-hotspot-keyboard"
                    cx={hotspot.cx}
                    cy={hotspot.cy}
                    rx={hotspot.rx}
                    ry={hotspot.ry}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                  />
                {:else if hotspot.kind === 'polygon'}
                  <polygon
                    class="scene-hotspot scene-hotspot-keyboard"
                    points={hotspot.points}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    stroke-linejoin="round"
                  />
                {/if}
              {/each}
            </g>
          {/if}

          {#if postCoverMouseHotspots.length}
            <g
              class="scene-interaction-layer post-cover-mouse-hotspot-layer"
              clip-path={mouseVisualClipUrl || undefined}
            >
              {#each postCoverMouseHotspots as hotspot}
                {#if hotspot.kind === 'rect'}
                  <rect
                    class="scene-hotspot scene-hotspot-mouse"
                    x={hotspot.x}
                    y={hotspot.y}
                    width={hotspot.width}
                    height={hotspot.height}
                    rx={hotspot.rx ?? 5}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    transform={hotspot.transform || undefined}
                  />
                {:else if hotspot.kind === 'ellipse'}
                  <ellipse
                    class="scene-hotspot scene-hotspot-mouse"
                    cx={hotspot.cx}
                    cy={hotspot.cy}
                    rx={hotspot.rx}
                    ry={hotspot.ry}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                  />
                {:else if hotspot.kind === 'polygon'}
                  <polygon
                    class="scene-hotspot scene-hotspot-mouse"
                    points={hotspot.points}
                    fill={hotspot.fill}
                    stroke={hotspot.stroke}
                    stroke-width="1.8"
                    stroke-linejoin="round"
                  />
                {/if}
              {/each}
            </g>
          {/if}

          {#if showKeyboardLayers && keyboardVisualSrc}
            <image
              href={keyboardVisualSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="scene-visual-layer keyboard-visual-layer"
              clip-path={keyboardVisualClipUrl || undefined}
            />
          {/if}

          {#if showMouseLayers && mouseVisualSrc}
            <image
              href={mouseVisualSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="scene-visual-layer mouse-visual-layer"
              clip-path={mouseVisualClipUrl || undefined}
            />
          {/if}

          {#if staticSceneMouseGeometry}
            <g
              class="scene-mouse-paw"
              clip-path={mouseVisualClipUrl || undefined}
              transform={`translate(${staticSceneMouseGeometry.pawX} ${staticSceneMouseGeometry.pawY}) rotate(${staticSceneMouseGeometry.pawAngle})`}
            >
              <ellipse
                cx="0"
                cy="6"
                rx="11"
                ry="9"
                fill={mousePointerIndicatorTone.pawFill}
                stroke={mousePointerIndicatorTone.pawStroke}
                stroke-width="2"
              />
              <circle cx="-8" cy="-4" r="3.6" fill={mousePointerIndicatorTone.pawFill} stroke={mousePointerIndicatorTone.pawStroke} stroke-width="1.6" />
              <circle cx="-2.5" cy="-8" r="3.4" fill={mousePointerIndicatorTone.pawFill} stroke={mousePointerIndicatorTone.pawStroke} stroke-width="1.6" />
              <circle cx="3.5" cy="-8" r="3.4" fill={mousePointerIndicatorTone.pawFill} stroke={mousePointerIndicatorTone.pawStroke} stroke-width="1.6" />
              <circle cx="9" cy="-4" r="3.6" fill={mousePointerIndicatorTone.pawFill} stroke={mousePointerIndicatorTone.pawStroke} stroke-width="1.6" />
              <ellipse cx="0" cy="7" rx="5.5" ry="4.4" fill={mousePointerIndicatorTone.padFill} />
              <circle cx="-7.6" cy="-4.2" r="1.35" fill={mousePointerIndicatorTone.padFill} />
              <circle cx="-2.4" cy="-7.9" r="1.25" fill={mousePointerIndicatorTone.padFill} />
              <circle cx="3.4" cy="-7.9" r="1.25" fill={mousePointerIndicatorTone.padFill} />
              <circle cx="8.8" cy="-4.2" r="1.35" fill={mousePointerIndicatorTone.padFill} />
            </g>
          {/if}

          {#if useSourceKeyboardMode && leftHandSrc}
            <image
              href={leftHandSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="keyboard-layer source-left-hand-layer active"
            />
          {/if}

          {#if useSourceKeyboardMode && rightHandSrc}
            <image
              href={rightHandSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="keyboard-layer source-right-hand-layer active"
            />
          {/if}

          {#if showMouseLayers && preset.showMouseDevice}
            <image
              href={mouseDeviceBaseSrc}
              x={mouseDeviceX}
              y={mouseDeviceY}
              width={DEVICE_WIDTH}
              height={DEVICE_HEIGHT}
              class="mouse-device-layer"
              style={mouseDeviceStyle}
            />

            {#if mouseDeviceOverlaySrc}
              <image
                href={mouseDeviceOverlaySrc}
                x={mouseDeviceX}
                y={mouseDeviceY}
                width={DEVICE_WIDTH}
                height={DEVICE_HEIGHT}
                class="mouse-device-layer mouse-device-overlay"
                style={mouseOverlayStyle}
              />
            {/if}
          {/if}

          {#if showMouseLayers && preset.showMouseArm}
            <polygon points={mouseArmPoints} class="mouse-arm-fill" style={mouseArmStyle} />
            <polyline points={mouseArmPoints} class="mouse-arm-shadow" style={mouseArmStyle} />
            <polyline points={mouseArmPoints} class="mouse-arm-stroke" style={mouseArmStyle} />
          {/if}

          {#if showKeyboardLayers && preset.showKeyboardOverlay && keyOverlaySrc}
            <image
              href={keyOverlaySrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="keyboard-layer keys-layer"
              class:key-pose-1={keyOverlayPose === 1}
              class:key-pose-2={keyOverlayPose === 2}
              style={keyboardOverlayStyle}
            />
          {/if}

          {#if showKeyboardLayers && !useSourceKeyboardMode}
            <image
              href={standardHandSrc}
              x="0"
              y="0"
              width={SCENE_WIDTH}
              height={SCENE_HEIGHT}
              class="keyboard-layer standard-hand-layer active"
              style={handStyle}
            />
          {/if}
        </g>
      </svg>
    </div>
  </div>
</div>

<style>
  .avatar-shell {
    display: flex;
    height: 100%;
    width: 100%;
    align-items: flex-end;
    justify-content: center;
    opacity: var(--avatar-shell-opacity);
    cursor: grab;
    transform-origin: center bottom;
    animation: avatar-float 3.1s ease-in-out infinite;
  }

  .avatar-shell:active {
    cursor: grabbing;
  }

  .scene-frame {
    position: relative;
    height: 100%;
    width: 100%;
  }

  .scene-svg {
    pointer-events: none;
    position: absolute;
    inset: 0;
    height: 100%;
    width: 100%;
    user-select: none;
    filter: drop-shadow(0 8px 12px rgba(15, 23, 42, 0.06));
  }

  .scene-base {
    position: relative;
  }

  .scene-svg image,
  .scene-svg polygon,
  .scene-svg polyline {
    pointer-events: none;
  }

  .static-cover-layer {
    transition: transform 95ms ease-out;
    transform-origin: center center;
  }

  .scene-visual-layer {
    transition: opacity 90ms linear;
    pointer-events: none;
  }

  .scene-interaction-layer {
    mix-blend-mode: normal;
  }

  .scene-hotspot {
    filter: drop-shadow(0 2px 8px rgba(34, 211, 238, 0.18));
  }

  .scene-hotspot-mouse {
    filter: drop-shadow(0 2px 8px rgba(236, 72, 153, 0.16));
  }

  .scene-hotspot-keyboard {
    filter: drop-shadow(0 2px 8px rgba(34, 211, 238, 0.22));
  }

  .scene-mouse-paw {
    filter: drop-shadow(0 2px 10px rgba(15, 23, 42, 0.2));
  }

  .keyboard-layer {
    transition: transform 90ms linear, opacity 90ms linear;
  }

  .keyboard-layer.active {
    opacity: 1;
  }

  .keys-layer {
    --key-y: 0px;
    transform: translateY(var(--key-y));
  }

  .mouse-device-layer {
    transition: transform 80ms linear, opacity 80ms linear;
  }

  .mouse-device-overlay {
    opacity: 0.96;
  }

  .mouse-arm-fill {
    fill: #fff;
  }

  .mouse-arm-shadow {
    fill: none;
    stroke: rgba(0, 0, 0, 0.3);
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-width: 7;
  }

  .mouse-arm-stroke {
    fill: none;
    stroke: #000;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-width: 6;
  }

  .key-pose-1 {
    opacity: 0.82;
    --key-y: 1px;
  }

  .key-pose-2 {
    opacity: 0.96;
    --key-y: 2px;
  }

  .idle-breathe {
    animation: idle-breathe 3.6s ease-in-out infinite;
  }

  .idle-sway {
    animation: idle-sway 4.2s ease-in-out infinite;
  }

  .idle-observe {
    animation: idle-observe 4.8s ease-in-out infinite;
  }

  .idle-groove {
    animation: idle-groove 2.8s ease-in-out infinite;
  }

  .idle-focus-pulse {
    animation: idle-focus-pulse 3.1s ease-in-out infinite;
  }

  .transition-alert {
    animation: avatar-float 3.1s ease-in-out infinite, transition-alert 0.72s ease-out;
  }

  .transition-settle {
    animation: avatar-float 3.1s ease-in-out infinite, transition-settle 0.68s ease-out;
  }

  .transition-snap-back {
    animation: avatar-float 3.1s ease-in-out infinite, transition-snap-back 0.76s ease-out;
  }

  .transition-focus-shift {
    animation: avatar-float 3.1s ease-in-out infinite, transition-focus-shift 0.64s ease-out;
  }

  .transition-glide {
    animation: avatar-float 3.1s ease-in-out infinite, transition-glide 0.62s ease-out;
  }

  .transition-lift {
    animation: avatar-float 3.1s ease-in-out infinite, transition-lift 0.66s ease-out;
  }

  @keyframes avatar-float {
    0%,
    100% {
      transform: translateY(0);
    }

    50% {
      transform: translateY(-2.4px);
    }
  }

  @keyframes idle-breathe {
    0%,
    100% {
      transform: translateY(0) scale(1);
    }

    50% {
      transform: translateY(-1.6px) scale(1.006);
    }
  }

  @keyframes idle-sway {
    0%,
    100% {
      transform: translateX(0) translateY(0);
    }

    50% {
      transform: translateX(-1.8px) translateY(-1.2px);
    }
  }

  @keyframes idle-observe {
    0%,
    100% {
      transform: translateY(0);
    }

    30% {
      transform: translate(-1px, -1.4px);
    }

    65% {
      transform: translate(1.4px, -0.6px);
    }
  }

  @keyframes idle-groove {
    0%,
    100% {
      transform: translateX(0) translateY(0) rotate(0deg);
    }

    25% {
      transform: translateX(-1px) translateY(-1.6px) rotate(-0.6deg);
    }

    75% {
      transform: translateX(1.2px) translateY(-1.1px) rotate(0.8deg);
    }
  }

  @keyframes idle-focus-pulse {
    0%,
    100% {
      transform: translateY(0) scale(1);
    }

    50% {
      transform: translateY(-1.8px) scale(1.01);
    }
  }

  @keyframes transition-alert {
    0% {
      transform: translateY(8px) scale(0.98);
      opacity: 0.76;
    }

    60% {
      transform: translateY(-3px) scale(1.015);
      opacity: 1;
    }

    100% {
      transform: translateY(0) scale(1);
      opacity: 1;
    }
  }

  @keyframes transition-settle {
    0% {
      transform: translateY(-4px) scale(1.015);
    }

    100% {
      transform: translateY(0) scale(1);
    }
  }

  @keyframes transition-snap-back {
    0% {
      transform: translateX(6px) scale(0.985);
    }

    45% {
      transform: translateX(-4px) scale(1.01);
    }

    100% {
      transform: translateX(0) scale(1);
    }
  }

  @keyframes transition-focus-shift {
    0% {
      transform: translateY(4px) scale(0.992);
    }

    55% {
      transform: translateY(-3px) scale(1.012);
    }

    100% {
      transform: translateY(0) scale(1);
    }
  }

  @keyframes transition-glide {
    0% {
      transform: translateX(-5px) translateY(2px);
      opacity: 0.84;
    }

    100% {
      transform: translateX(0) translateY(0);
      opacity: 1;
    }
  }

  @keyframes transition-lift {
    0% {
      transform: translateY(8px) scale(0.985);
    }

    100% {
      transform: translateY(0) scale(1);
    }
  }
</style>
