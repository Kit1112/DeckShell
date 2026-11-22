// Render the stock SVGs used by Gaming Mode's Settings and controller UI.
// Generate into Cargo OUT_DIR so neither Valve assets nor a stale cache are needed in Git.
import fs from 'node:fs';
import path from 'node:path';
import { modules, exportNames } from './lib.mjs';
import { evalModule, renderIcon } from './sandbox.mjs';

const [, , bundle, out] = process.argv;
const stockModules = [...modules([path.basename(bundle)], path.dirname(bundle))];
const lib = stockModules.find(({ src }) => exportNames(src).includes('WirelessNetwork'));
if (!lib) throw new Error(`Steam icon module missing from ${bundle}`);

// Steam's EWirelessStrength values from module 20887. Provide the enum to the
// JSX stub so component-level opacity is evaluated exactly as in Steam.
const strengths = { None: 0, Weak: 1, Ok: 2, Good: 3, Excellent: 4 };
const icons = evalModule((0, eval)(`(${lib.src})`), id =>
  id === 20887 ? { oH: strengths } : undefined);
function save(name, component = name, props = {}, ink = "currentColor") {
  saveSvg(name, renderIcon(icons[component], props), ink);
}
function saveSvg(name, svg, ink = "currentColor") {
  if (!svg?.startsWith('<svg ') || !svg.includes('viewBox=')) {
    throw new Error(`Could not extract ${name} from ${bundle}`);
  }
  fs.writeFileSync(path.join(out, `${name}.svg`), svg.replace(/currentcolor/gi, ink) + '\n');
}

save('CaratDown', 'Carat', { direction: 'down' });
save('LockClosed', 'Lock', { locked: true });
for (const [name, strength] of Object.entries(strengths)) {
  save(`WirelessNetwork-${strength}`, 'WirelessNetwork', { strength });
}
for (const name of [
  'SteamPal', 'WiredNetwork', 'Checkmark', 'WirelessDisabled', 'WirelessOfflineMode',
  'Close', 'Caution', 'WiFi', 'Spinner', 'FeatureVisible', 'FeatureHidden', 'AirplaneMode', 'Add', 'NightMode',
]) {
  save(name);
}

save('WirelessOfflineModeFocused', 'WirelessOfflineMode', {}, '#23262e');

// Settings rail icons: the exact components of the stock page table (`Ho` in sp.js, `#Settings_Page_*` entries), rendered
// like the Internet glyphs above. System uses SteamDeck on a Deck (Lo()); Audio is `AudioVolumeIcon flVolume ?? 1` (module 40572).
const railProps = { Lock: { locked: true }, AudioVolumeIcon: { flVolume: 1 } };
for (const name of [
  'SteamDeck', 'Lock', 'ExclamationPoint', 'Display', 'Lightning', 'AudioVolumeIcon', 'Bluetooth', 'ControllerStatus',
  'Keyboard', 'CustomizeSteamDeck', 'Friends', 'Download', 'CloudSync', 'FamilySharing', 'RemotePlay', 'HardDrive',
  'Home', 'Library', 'Shader', 'SteamLogo', 'ContactInfo', 'InGame', 'Music', 'Broadcast', 'Microphone', 'Security',
  'Accessibility', 'ShoppingCart',
  // Bluetooth device icons (stock `D(A)`, sp.js:345501)
  'Headphones', 'Mobile', 'Volume100',
]) {
  save(`Rail-${name}`, name, railProps[name] ?? {});
}

// Settings → «Запись игры» rail icon: `p.vN` of module 15786 (self-contained inline SVG, sp.js bundle).
{
  const rec = stockModules.find(({ id }) => id === '15786');
  if (!rec) throw new Error(`Game recording icon module 15786 missing from ${bundle}`);
  const svg = renderIcon(evalModule((0, eval)(`(${rec.src})`), () => undefined).vN, {});
  if (!svg?.includes('viewBox=') || !svg.includes('<path')) throw new Error('Could not extract the Game recording icon');
  fs.writeFileSync(path.join(out, 'Rail-GameRecording.svg'), svg + '\n');
}

// The layout browser, device tabs and input-mode menu use these stock components.
const controllerEnum = stockModules.find(({ src }) =>
  ['AE', 'D$', 'rb', 'HD', 'UK'].every(key => exportNames(src).includes(key)));
const svgIdHook = stockModules.find(({ src }) => src.includes('"svgid_') && exportNames(src).includes('l'));
if (!controllerEnum || !svgIdHook) throw new Error('Steam controller icon dependencies not found');
const controllerTypes = evalModule((0, eval)(`(${controllerEnum.src})`));
const controllerIcons = evalModule((0, eval)(`(${lib.src})`), id => {
  if (String(id) === controllerEnum.id) return controllerTypes;
  if (String(id) === svgIdHook.id) return { l: () => ['controller-clip', 'url(#controller-clip)'] };
});
for (const [name, typeKey] of Object.entries({
  Deck: 'AE', Xbox: 'D$', Steam: 'rb', Triton: 'VD', PS4: '_X', PS5: 'HD', Switch: 'gW',
  Hori: 'tE', Mobile: 'kL', Frame: 'My', Generic: 'UK',
})) {
  const svg = renderIcon(controllerIcons.ControllerType, {
    controllerType: controllerTypes[typeKey],
  });
  // Rotate the standalone phone around its viewBox center so it stays inside the thumbnail.
  const standalone = name === 'Mobile'
    ? svg?.replace('matrix(1, 0, 0, 1, 0, 0)rotate(270)', 'rotate(270 32 32)')
    : svg;
  saveSvg(`ControllerType.${name}`, standalone);
}
for (const name of [
  'Mouse', 'GuideButton', 'SteamInputModeScroll2D', 'SteamInputModeScrollWheel',
  'TouchTap', 'RadialMenu', 'TouchMenu', 'HotbarMenu', 'User', 'Edit', 'Star', 'SteamLogo',
  'Search', 'ThumbsUp', 'Clock', 'Download',
]) {
  const svg = renderIcon(controllerIcons[name], {});
  // The stock text-bullets path inherits its fill from the Steam UI stylesheet.
  saveSvg(name, name === 'SteamInputModeScrollWheel' ? svg?.replace('fill="none"', 'fill="currentColor"') : svg);
}
