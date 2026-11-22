// Pure battery decisions transcribed 1:1 from stock Steam UI (tools/cdp/build/pretty/sp.js, 2026-11-18 build).
// Only state -> value; no timers, MobX or DOM. Enums: confidence Low=0 Medium=1 High=2 (sp.js:425893);
// battery state Unknown=0 Discharging=1 Charging=2 Full=3 (module 30677); AC state 0..3, 3 = "connected slow" (module 98146).

// sp.js:425962-425977 OnBatteryStateChanged: an AC state change drops confidence to Low (0).
export function confidenceOnBatteryUpdate(prevAc, newAc, confidence) {
  return newAc !== prevAc ? 0 : confidence;
}

// sp.js:425965-425974: Low -> (10 s) -> Medium -> (1 min) -> High. Returns the delay in ms until the next step, -1 at High.
export function confidenceTimerDelayMs(confidence) {
  if (confidence === 0) return 10000;
  if (confidence === 1) return 60000;
  return -1;
}

// The step the timer takes when it fires.
export function confidenceAfterTimer(confidence) {
  if (confidence === 0) return 1;
  return 2;
}

// sp.js:425975-425985: "say full" latch. Full(3) sets it at once; leaving Full clears it after 5 s; otherwise unchanged.
export function sayFullAction(prevBatteryState, newBatteryState) {
  if (newBatteryState === 3) return "set";
  if (prevBatteryState === 3) return "clear-after-5000";
  return "keep";
}

// sp.js:406630-406637: hours/minutes of the QAM «Projected battery life» value; "?" until the seconds are known.
export function batteryTimeParts(secondsRemaining) {
  let h = "?", m = "?";
  if (isFinite(secondsRemaining) && secondsRemaining >= 0) {
    h = Math.floor(secondsRemaining / 60 / 60).toString();
    m = Math.floor((secondsRemaining / 60) % 60).toString();
  }
  return h + "|" + m;
}

// sp.js:406636-406637: the value line is shown above Low confidence, otherwise a zero-width space.
export function batteryTimeShown(confidence) {
  return confidence > 0;
}

// sp.js:406638-406653: caption token under the time (battery state 1 = discharging, 2 = charging); "" = whole block hidden.
export function batteryTimeCaptionToken(batteryState, confidence) {
  switch (batteryState) {
    case 1:
      return confidence < 2 ? "#QuickAccess_Tab_Perf_CalculatingBatteryLife" : "#QuickAccess_Tab_Perf_ProjectedBatteryLife";
    case 2:
      return confidence < 2 ? "#QuickAccess_Tab_Perf_CalculatingChargingTime" : "#QuickAccess_Tab_Perf_ProjectedChargingTime";
    default:
      return "";
  }
}

// sp.js:401440-401441 (header Tr): level 0..1 -> "NN%" capped at 99; "" when the level is not finite.
export function batteryPercentText(level) {
  const p = isFinite(level) ? Math.min(Math.round(level * 100), 99) : Infinity;
  return isFinite(p) ? p + "%" : "";
}

// sp.js:401423 + 401500-401506: whether the header shows the percentage (region 2 never; full needs the visibility hint).
export function showHeaderPercent(prefShow, neverShow, region, isFull, hintVisible) {
  const un = region != 2 && prefShow && !neverShow;
  return un && (!isFull || hintVisible === true);
}

// sp.js:455129-455134 / 454945: stored preference object -> effective flag (absent key means off).
export function batteryPrefShow(storedJson) {
  let o;
  try { o = JSON.parse(storedJson); } catch (e) { return false; }
  if (o === null || typeof o !== "object") return false;
  return o.bShowBatteryPercentage ? true : false;
}

// sp.js:550419-550431: toggle result — the object the settings page stores when the row is switched to `v`.
export function batteryPrefStored(v) {
  return JSON.stringify({ bShowBatteryPercentage: v ? true : false });
}
