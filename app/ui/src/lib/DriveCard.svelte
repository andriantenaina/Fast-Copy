<script lang="ts">
  import Icon from './Icon.svelte';
  import { t, fmtBytes } from './i18n.svelte';
  import type { VolumeView } from './api';

  let { v, selected = false, onpick }: { v: VolumeView; selected?: boolean; onpick: () => void } = $props();

  const icon = $derived(v.network ? 'network' : v.bus === 'sd' ? 'sd' : v.external ? 'usb' : 'drive');
  const used = $derived(v.totalBytes && v.freeBytes != null ? 1 - v.freeBytes / v.totalBytes : 0);
  const name = $derived(v.label || (v.external ? t('drive.external') : t('drive.local')));
</script>

<button class="drive" class:selected aria-pressed={selected} onclick={onpick} title={v.model || v.root}>
  <div class="top">
    <span class="ic" class:ext={v.external}><Icon name={icon} size={18} /></span>
    <span class="badge fs-{v.fs}">{v.fsLabel}</span>
  </div>
  <div class="name">{name}</div>
  <div class="root muted mono">{v.root}</div>
  {#if v.totalBytes}
    <div class="bar" class:warn={used > 0.9} aria-hidden="true"><i style="width:{Math.round(used * 100)}%"></i></div>
    <div class="free muted num">{t('drive.free', { free: fmtBytes(v.freeBytes), total: fmtBytes(v.totalBytes) })}</div>
  {/if}
  {#if v.readOnly}<div class="ro sev-warning">{t('drive.readOnly')}</div>{/if}
</button>

<style>
  .drive {
    flex: none;
    width: 168px;
    display: grid;
    gap: 4px;
    padding: 12px;
    text-align: left;
    border-radius: 13px;
    border: 1px solid var(--line);
    background: rgba(5, 8, 24, 0.4);
    color: var(--text);
    cursor: pointer;
    scroll-snap-align: start;
    transition: transform 0.25s var(--ease), border-color 0.2s, box-shadow 0.3s, background 0.25s;
  }
  .drive:hover {
    transform: translateY(-2px);
    border-color: var(--line-strong);
    box-shadow: 0 14px 30px -18px rgba(34, 211, 238, 0.5);
  }
  .drive.selected {
    border-color: rgba(34, 211, 238, 0.7);
    background: linear-gradient(160deg, rgba(34, 211, 238, 0.14), rgba(167, 139, 250, 0.08));
    box-shadow: 0 0 0 1px rgba(34, 211, 238, 0.3) inset, 0 0 30px -10px rgba(34, 211, 238, 0.6);
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    color: var(--text-2);
    background: rgba(160, 175, 255, 0.08);
  }
  .ic.ext {
    color: var(--cyan);
    background: rgba(34, 211, 238, 0.12);
  }
  .name {
    font-weight: 650;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .root {
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-bottom: 6px;
  }
  .free {
    font-size: 11.5px;
    margin-top: 4px;
  }
  .ro {
    font-size: 11.5px;
    font-weight: 600;
  }
</style>
