import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/screens/controller_devices.js', import.meta.url), 'utf8');
const deck = {
    nControllerIndex: 15, eControllerType: 4, strSerialNumber: 'test-deck',
    strName: 'Test Deck', unCapabilities: (1n << 39n) | (1n << 13n),
    bHaptics: false, nLStickDeadzone: 0,
};
const calls = [];
const manager = {
    StartControllerStateFlow(args) { calls.push(['start-state', args.controller_index, args.flow_mode]); return flowReply; },
    RegisterForNotifyGyroCalibrationStateChanged(report) {
        calibration = (body) => report({ Body: () => ({ toObject: () => body }) });
        return { unregister() { calibration = undefined; } };
    },
    StartGyroSoftwareCalibration(args) { calls.push(['start-gyro', args.controller_index, args.side]); return true; },
    CancelGyroSoftwareCalibration(args) { calls.push(['cancel-gyro', args.controller_index, args.side]); return true; },
    EndControllerStateFlow(args) { calls.push(['end-state', args.controller_index, args.flow_mode]); return true; },
};
let device = deck;
let callback;
let calibration;
let hardwareReply;
let flowReply = true;
let rejectSave = false;
let time = 0;
let watchdog;
const modules = {
    744: { f_: manager },
    40214: { ui: () => 8192 },
};
const window = {
    settingsStore: { m_ClientSettings: { controller_power_off_timeout: 15 } },
    webpackChunksteamui: { push([, , callback]) { callback(id => modules[id]); } },
    ControllerStore: {
        m_nSupportCounter: 0,
        GetController: index => device?.nControllerIndex === index ? device : null,
        GetControllers: () => device ? [device] : [],
        SetCalibratingIndex(index) { calls.push(['index', index]); },
        ControllerFeedStates: () => ({ 15: { m_controllerAxesState: { LeftJoystick: { x: 0, y: -0.5 } } } }),
    },
};
const input = {
    StartControllerDeviceSupportFlow(flow, index, report) {
        calls.push(['start', flow, index]);
        callback = report;
    },
    EndControllerDeviceSupportFlow() { calls.push(['end']); },
    InitializeControllerPersonalizationSettings(idx) { calls.push(['initialize', idx]); },
    SetControllerPersonalizationSetting(key, val) { calls.push(['set', key, val]); },
    SaveControllerPersonalizationSettings(idx) { calls.push(['save', idx]); },
    SaveControllerCalibration(idx) {
        calls.push(['save-calibration', idx]);
        if (rejectSave) throw new Error('save rejected');
    },
    CalibrateControllerIMU(idx) {
        calls.push(['start-hardware', idx]);
        return hardwareReply;
    },
};
const env = {
    window, SteamClient: { Input: input, Settings: {} },
    performance: { now: () => time },
    setTimeout(fn) { watchdog = fn; return 1; }, clearTimeout() { watchdog = undefined; },
};
function deviceText(locale) {
    const catalog = JSON.parse(readFileSync(new URL(`../../locales/${locale}.json`, import.meta.url), 'utf8'));
    return Object.fromEntries(Object.entries(catalog)
        .filter(([key]) => key.startsWith('controller.devices.'))
        .map(([key, value]) => [key.split('.').at(-1), value]));
}

const russianText = deviceText('ru');
const request = async (data, text = russianText) => JSON.parse(JSON.stringify(await runInNewContext(
    source.replace('/*text*/', JSON.stringify(text)).replace('/*request*/', JSON.stringify(data)), env)));

const id = { index: 15, type: 4, serial: 'test-deck' };

const inventory = await request({ op: 'inventory', defaults: true });
assert.equal(inventory.idle, 15);
assert.equal(inventory.devices[0].caps, deck.unCapabilities.toString());
assert.equal(inventory.devices[0].bHaptics, false);
assert.equal(inventory.devices[0].nLStickDeadzone, 0);
assert.equal(inventory.devices[0].defaultDeadzone, 8192);
assert.equal('nRStickDeadzone' in inventory.devices[0], false);

const feed = { ...id, op: 'feed', mode: 'test', token: 17 };
assert.deepEqual(await request(feed), { sample: null, exit: false });
assert.deepEqual(calls.shift(), ['start', 2, 15]);
callback({ LeftJoystickX: 1200, LeftJoystickY: -42, bButtonEast: true });
time = 749;
assert.deepEqual(await request(feed), {
    sample: { LeftJoystickX: 1200, LeftJoystickY: -42, bButtonEast: true }, exit: false,
});
callback({ bButtonEast: false });
time = 800;
assert.equal((await request(feed)).exit, false);
callback({ bButtonEast: true });
time = 1550;
assert.equal((await request(feed)).exit, true);
await request({ op: 'end', token: feed.token });
assert.deepEqual(calls.splice(0), [['end']]);
assert.equal(await request(feed), null, 'a late poll cannot reopen a closed feed');
assert.equal(watchdog, undefined);

const sticks = { ...id, op: 'feed', mode: 'sticks', token: 18 };
assert.deepEqual(await request(sticks), { axes: { left: { x: 0, y: -0.5 } } });
assert.deepEqual(calls.splice(0), [['index', 15], ['start-state', 15, 1]]);
await watchdog();
assert.deepEqual(calls.splice(0), [['end-state', 15, 1]]);

await request({ ...id, op: 'personalization', key: 'nLStickDeadzone', value: 8800 });
assert.deepEqual(calls.splice(0), [
    ['initialize', 15], ['set', 'nLStickDeadzone', 8800], ['save', 15], ['save-calibration', 15],
]);
device = { ...device, strSerialNumber: 'another-device' };
await assert.rejects(request({ ...id, op: 'personalization', key: 'nLStickDeadzone', value: 8800 }), /заменён/);
assert.deepEqual(calls, []);
const gyroCap = 1n << 11n;
const uncalibratedCap = 1n << 18n;
await assert.rejects(request({ ...id, op: 'gyro-target' }), /заменён/);
device = { ...deck };
await assert.rejects(request({ ...id, op: 'gyro-target' }), /гироскоп/i);
device.unCapabilities = gyroCap | uncalibratedCap;
assert.deepEqual(await request({ ...id, op: 'gyro-target' }), {
    nControllerIndex: 15, eControllerType: 4, strSerialNumber: 'test-deck',
    name: 'Test Deck', mode: 'software',
});
const software = { ...id, mode: 'software', token: 'test-imu-software' };
const sw = op => request({ ...software, op: 'gyro-' + op });
const emit = (calibration_state, overrides = {}) => calibration({
    controller_index: 15, imu_index: 1, calibration_state, calibration_progress: 0.5,
    accelerometer_noise_tolerance: 1.2, gyroscope_noise_tolerance: 3.4, ...overrides,
});
await sw('start');
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['start-gyro', 15, 1]]);
await assert.rejects(request({ ...software, token: 'other', op: 'gyro-start' }), /уже выполняется/);
emit(3); // A terminal event without a new working stage cannot confirm the attempt.
emit(1, { controller_index: 2 });
emit(2, { imu_index: 0 });
assert.deepEqual(await sw('poll'), { stage: 'waiting' });
await assert.rejects(sw('save'), /допуски/);
emit(1);
assert.deepEqual(await sw('poll'), { stage: 'stationary', progress: 0.5 });
emit(2, { calibration_progress: 0.75 });
assert.deepEqual(await sw('poll'), { stage: 'drift', progress: 0.75 });
emit(3, { imu_index: 0 });
assert.deepEqual(await sw('poll'), { stage: 'drift', progress: 0.75 });
emit(3);
assert.deepEqual(await sw('poll'), { stage: 'complete', savable: true });
assert.deepEqual(calls, [], 'completion alone does not save');
await sw('save');
assert.equal(device.flAccelerometerStationaryTolerance, 1.2);
assert.equal(device.flGyroStationaryTolerance, 3.4);
await assert.rejects(sw('save'), /допуски/);
await sw('end');
assert.equal(calibration, undefined);
assert.deepEqual(calls.splice(0), [['save-calibration', 15], ['cancel-gyro', 15, 1], ['end-state', 15, 5]]);

await sw('start');
emit(4);
assert.deepEqual(await sw('poll'), {
    stage: 'failed', detail: 'Steam сообщил об ошибке калибровки', retryable: true,
});
await watchdog();
assert.equal(calibration, undefined, 'the watchdog releases the registration');
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['start-gyro', 15, 1],
    ['cancel-gyro', 15, 1], ['end-state', 15, 5]]);

await sw('start');
emit(1);
emit(3, { accelerometer_noise_tolerance: 0 });
assert.deepEqual(await sw('poll'), { stage: 'complete', savable: false });
await assert.rejects(sw('save'), /допуски/);
await sw('end');
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['start-gyro', 15, 1],
    ['cancel-gyro', 15, 1], ['end-state', 15, 5]]);

await sw('start');
emit(1);
emit(3, { accelerometer_noise_tolerance: 2.5, gyroscope_noise_tolerance: 6 });
rejectSave = true;
await assert.rejects(sw('save'), /save rejected/);
assert.equal(device.flAccelerometerStationaryTolerance, 1.2);
assert.equal(device.flGyroStationaryTolerance, 3.4);
rejectSave = false;
await sw('end');
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['start-gyro', 15, 1],
    ['save-calibration', 15], ['cancel-gyro', 15, 1], ['end-state', 15, 5]]);

let releaseFlow;
flowReply = new Promise(resolve => { releaseFlow = resolve; });
await sw('start');
const closing = sw('end');
releaseFlow(true);
await closing;
flowReply = true;
assert.equal(calibration, undefined);
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['end-state', 15, 5]]);

await sw('start');
device = { ...device, strSerialNumber: 'replacement' };
await assert.rejects(sw('poll'), /заменён/);
await assert.rejects(sw('end'), /заменён/);
assert.equal(calibration, undefined);
await assert.rejects(sw('start'), /заменён/);
assert.deepEqual(calls.splice(0), [['start-state', 15, 5], ['start-gyro', 15, 1]]);

device = { ...deck, unCapabilities: gyroCap };
assert.equal((await request({ ...id, op: 'gyro-target' })).mode, 'hardware');
const hw = op => request({ ...id, mode: 'hardware', token: 'test-imu-hardware', op: 'gyro-' + op });
await hw('start');
time += 1000;
assert.deepEqual(await hw('poll'), { stage: 'hardware', progress: 0.5 });
time += 1000;
assert.deepEqual(await hw('poll'), { stage: 'unconfirmed' });
await hw('end');
assert.deepEqual(calls.splice(0), [['start-hardware', 15]]);
hardwareReply = false;
await hw('start');
assert.equal((await hw('poll')).stage, 'failed');
await hw('end');
assert.deepEqual(calls.splice(0), [['start-hardware', 15]]);
hardwareReply = undefined;

device = { ...device, eControllerType: 10, unCapabilities: gyroCap | uncalibratedCap, bUseSWBiases: false };
const triton = { ...id, type: 10, mode: 'hardware', token: 'test-imu-triton' };
assert.equal((await request({ ...triton, op: 'gyro-target' })).mode, 'hardware');
await request({ ...triton, op: 'gyro-start' });
time += 2000;
assert.deepEqual(await request({ ...triton, op: 'gyro-poll' }), { stage: 'hardware', progress: 0.5 });
time += 2000;
assert.deepEqual(await request({ ...triton, op: 'gyro-poll' }), { stage: 'unconfirmed' });
await request({ ...triton, op: 'gyro-end' });
assert.deepEqual(calls.splice(0), [['start-hardware', 15]]);
device.bUseSWBiases = true;
assert.equal((await request({ ...triton, op: 'gyro-target' })).mode, 'software');
await assert.rejects(request({ ...triton, op: 'gyro-start' }), /Режим калибровки изменился/);
assert.deepEqual(calls, []);

device = { ...deck, unCapabilities: gyroCap, strSerialNumber: '' };
const noSerial = { ...id, serial: '', mode: 'hardware', token: 'test-imu-no-serial' };
await request({ ...noSerial, op: 'gyro-target' });
device = { ...device };
await assert.rejects(request({ ...noSerial, op: 'gyro-start' }), /без серийного номера/);
assert.deepEqual(calls, []);
await request({ ...noSerial, op: 'gyro-target' });
await request({ ...noSerial, op: 'gyro-start' });
await request({ ...noSerial, op: 'gyro-end' });
assert.deepEqual(calls.splice(0), [['start-hardware', 15]]);

window.ControllerStore = undefined;
await assert.rejects(request({ op: 'inventory' }), /ControllerStore недоступен/);
await assert.rejects(request({ op: 'inventory' }, deviceText('en')), /ControllerStore unavailable/);

console.log('Controller devices: inventory, selected device identity, live feed, 750 ms hold, cleanup, personalization and IMU wizard passed');
