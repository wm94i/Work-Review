<script lang="ts">
  import Orbit from 'lucide-svelte/icons/orbit';
  import { locale, t } from '$lib/i18n/index.ts';
</script>

<main
  class="evidence-shell"
  data-locale={$locale}
  aria-label={t('evidenceTemplate.workspace')}
>
  <div class="evidence-shell__field" aria-hidden="true">
    <span class="evidence-shell__grid"></span>
    <span class="evidence-shell__arc evidence-shell__arc--one"></span>
    <span class="evidence-shell__arc evidence-shell__arc--two"></span>
    <Orbit size={240} strokeWidth={0.45} />
  </div>

  <aside class="evidence-shell__navigation">
    <slot name="navigation" />
  </aside>

  <section class="evidence-shell__stage">
    <slot />
  </section>

  <aside class="evidence-shell__inspector">
    <slot name="inspector" />
  </aside>
</main>

<style>
  .evidence-shell {
    position: relative;
    display: grid;
    grid-template-columns: 4.5rem minmax(0, 1fr) 2.25rem;
    width: 100%;
    height: 100%;
    min-height: 0;
    padding: 2rem 0.75rem 0.75rem;
    overflow: hidden;
    color: var(--evidence-text);
    background:
      radial-gradient(circle at 72% 18%, var(--evidence-shell-cyan-glow), transparent 28rem),
      radial-gradient(circle at 18% 82%, var(--evidence-shell-violet-glow), transparent 30rem),
      var(--evidence-canvas);
  }

  .evidence-shell__field {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
    color: var(--evidence-field-orbit);
    pointer-events: none;
  }

  .evidence-shell__field > :global(svg) {
    animation: evidence-orbit 28s linear infinite;
  }

  .evidence-shell__grid {
    position: absolute;
    inset: 0;
    opacity: 0.28;
    background-image:
      linear-gradient(var(--evidence-grid-line) 1px, transparent 1px),
      linear-gradient(90deg, var(--evidence-grid-line) 1px, transparent 1px);
    background-size: 48px 48px;
    mask-image: radial-gradient(circle at center, var(--evidence-mask-core), transparent 78%);
  }

  .evidence-shell__arc {
    position: absolute;
    width: min(62vw, 48rem);
    aspect-ratio: 1;
    border: 1px solid var(--evidence-arc-acid);
    border-radius: var(--radius-full);
  }

  .evidence-shell__arc--one {
    transform: translate(28%, -34%);
  }

  .evidence-shell__arc--two {
    width: min(42vw, 34rem);
    transform: translate(-74%, 48%);
    border-color: var(--evidence-arc-cyan);
  }

  .evidence-shell__navigation,
  .evidence-shell__stage,
  .evidence-shell__inspector {
    position: relative;
    z-index: 1;
    min-width: 0;
    min-height: 0;
  }

  .evidence-shell__stage {
    overflow: hidden;
    border: 1px solid var(--evidence-line-control);
    border-radius: var(--radius-lg);
    background: var(--evidence-stage-background);
    box-shadow: var(--evidence-stage-shadow);
    backdrop-filter: blur(18px);
  }

  .evidence-shell__inspector {
    border-inline-start: 1px solid var(--evidence-line-soft);
  }

  @keyframes evidence-orbit {
    to { transform: rotate(360deg); }
  }

  @media (max-width: 900px) {
    .evidence-shell {
      grid-template-columns: 3.75rem minmax(0, 1fr);
      padding-inline-end: 0.5rem;
    }

    .evidence-shell__inspector {
      display: none;
    }
  }

  @media (max-width: 640px) {
    .evidence-shell {
      grid-template-columns: 3.25rem minmax(0, 1fr);
      padding-inline: 0.25rem;
    }

    .evidence-shell__stage {
      border-radius: var(--radius-lg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .evidence-shell__field > :global(svg) {
      animation: none;
    }
  }
</style>
