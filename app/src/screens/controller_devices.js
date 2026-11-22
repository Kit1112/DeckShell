// Runs in SharedJSContext. These are the same native services used by the settings,
// calibration and device-support pages in Steam's sp.js.
(async (req) => {
    const text = /*text*/;
    const store = window.ControllerStore;
    if (!store) throw new Error(text.controllerstore_unavailable);
    const input = SteamClient.Input;
    const settings = SteamClient.Settings;
    const checked = (reply, method) => {
        if (reply === false || (typeof reply?.BSuccess === "function" && !reply.BSuccess()))
            throw new Error(method + ": " + (reply?.GetEResult?.() ?? text.steam_rejected_the_request));
        return reply;
    };
    const device = () => {
        const d = store.GetController(req.index);
        if (!d || d.eControllerType !== req.type || (d.strSerialNumber ?? "") !== req.serial)
            throw new Error(text.controller_disconnected_or_replaced);
        return d;
    };
    const hasGyro = d => (BigInt(d?.unCapabilities ?? 0) & (1n << 11n)) !== 0n;
    const native = async (method, args) => checked(await input[method](...args), method);
    const service = (module, key) => {
        let require;
        window.webpackChunksteamui.push([["deckshell-controller-" + Math.random()], {}, r => { require = r; }]);
        return require(module)[key];
    };
    const manager = () => service(744, "f_");
    const rpc = async (api, method, args) => checked(await api[method](args), method);
    const slot = "__deckshellControllerFeed";
    const ended = "__deckshellControllerFeedEnded";
    // sp.js module 77519: Triton uses the wizard with software biases; other
    // devices use it when Steam advertises an uncalibrated IMU.
    const gyroMode = d => !hasGyro(d) ? null :
        (d.eControllerType === 10 ? d.bUseSWBiases : (BigInt(d.unCapabilities) & (1n << 18n)) !== 0n)
            ? "software" : "hardware";
    const gyroSlot = "__deckshellGyroCalibration";
    const gyroPrompt = "__deckshellGyroPrompt";
    const gyroSide = 1; // Configurator main gyro (sp.js:356715).
    const finitePositive = value => Number.isFinite(value) && value > 0;
    const gyroSession = () => {
        const session = window[gyroSlot];
        if (!session || session.closed || session.token !== req.token)
            throw new Error(text.steam_lost_the_calibration_session);
        return session;
    };
    const fields = [
        "nControllerIndex", "eControllerType", "strSerialNumber", "strName",
        "bBluetooth", "bWireless", "eRumblePreference", "bHaptics", "bUseReversedLayout",
        "bUseUniversalFaceButtonGlyphs", "nLStickDeadzone", "nRStickDeadzone",
        "bLeftStickTouchDisablesLeftTrackPad", "bRightStickTouchDisablesRightTrackPad",
        "nLHapticStrength", "nRHapticStrength", "unVendorID", "unProductID",
        "bSWAntiDrift", "bUseSWBiases"
    ];
    switch (req.op) {
        case "inventory": {
            const devices = store.GetControllers().map(d => Object.assign(
                Object.fromEntries(fields.filter(k => d[k] != null).map(k => [k, d[k]])),
                { caps: d.unCapabilities?.toString(),
                  defaultDeadzone: req.defaults ? service(40214, "ui")(d.eControllerType, d.unVendorID, d.unProductID) : undefined }
            ));
            const dongles = req.dongles
                ? (await rpc(manager(), "GetDongles", {})).Body().toObject()
                : null;
            return { devices, idle: window.settingsStore?.m_ClientSettings?.controller_power_off_timeout ?? null, dongles };
        }
        case "feed": {
            device();
            if (window[ended]?.has(req.token)) return null;
            let lease = window[slot];
            if (lease?.token !== req.token) {
                if (lease) await lease.close();
                if (req.mode === "test" && store.m_nSupportCounter !== 0)
                    throw new Error(text.steam_is_already_running_a_controller_test);
                const api = req.mode === "test" ? null : manager();
                const modes = req.mode === "gyro" ? [4] : [1];
                const started = [];
                lease = {
                    token: req.token, sample: null, heldSince: null, timer: null,
                    async close() {
                        if (window[slot] !== this) return;
                        delete window[slot];
                        clearTimeout(this.timer);
                        if (!api) {
                            await native("EndControllerDeviceSupportFlow", []);
                        } else {
                            const results = await Promise.allSettled(started.map(args => rpc(api, "EndControllerStateFlow", args)));
                            const error = results.find(result => result.status === "rejected");
                            if (error) throw error.reason;
                        }
                    }
                };
                window[slot] = lease;
                try {
                    if (!api) {
                        await native("StartControllerDeviceSupportFlow", [2, req.index, sample => {
                            lease.sample = { ...sample };
                            if (sample.bButtonEast) lease.heldSince ??= performance.now();
                            else lease.heldSince = null;
                        }]);
                    } else {
                        store.SetCalibratingIndex(req.index);
                        for (const mode of modes) {
                            const args = { controller_index: req.index, flow_mode: mode };
                            started.push(args);
                            await rpc(api, "StartControllerStateFlow", args);
                        }
                    }
                } catch (error) {
                    await lease.close().catch(() => {});
                    throw error;
                }
            }
            clearTimeout(lease.timer);
            // Also release the native flow if the app or CDP connection goes away.
            lease.timer = setTimeout(() => lease.close().catch(console.error), 1800);
            if (req.mode === "test") {
                return {
                    sample: lease.sample,
                    exit: lease.heldSince !== null && performance.now() - lease.heldSince >= 750
                };
            }
            const state = store.ControllerFeedStates()[req.index];
            const axes = state?.m_controllerAxesState;
            const gyro = state?.m_controllerGyroSpeedState?.GyroRawSpeed;
            return { axes: axes ? { left: axes.LeftJoystick, right: axes.RightJoystick } : null, gyro };
        }
        case "end":
            (window[ended] ??= new Set()).add(req.token);
            setTimeout(() => window[ended].delete(req.token), 10000);
            if (window[slot]?.token === req.token) await window[slot].close();
            break;
        case "name":
            device();
            await native("SetControllerName", [req.index, req.value]);
            break;
        case "identify":
            device();
            await native("IdentifyController", [req.index]);
            break;
        case "toggle": {
            device();
            const [target, method] = {
                rumble: [input, "SetControllerRumbleSetting"],
                haptics: [input, "SetControllerHapticSetting"],
                nintendo: [settings, "SetUseNintendoButtonLayout"],
                universal: [settings, "SetUseUniversalFaceButtonGlyphs"]
            }[req.key];
            checked(await target[method](req.index, req.value), method);
            break;
        }
        case "personalization":
            device();
            await native("InitializeControllerPersonalizationSettings", [req.index]);
            await native("SetControllerPersonalizationSetting", [req.key, req.value]);
            await native("SaveControllerPersonalizationSettings", [req.index]);
            await native("SaveControllerCalibration", [req.index]);
            break;
        case "pulse":
            device();
            await native("ForceSimpleHapticEvent", [req.index, req.key === "nLHapticStrength" ? 0 : 1, 2, 0, req.value]);
            break;
        case "idle": {
            const message = service(29303, "Ne");
            const Writer = service(58663, "BinaryWriter");
            const writer = new Writer();
            message.serializeBinaryToWriter(message.fromObject({controller_power_off_timeout: req.value}), writer);
            checked(await settings.SetSetting(writer.getResultBase64String()), "SetSetting");
            break;
        }
        case "firmware": {
            const api = service(45789, "BP");
            await rpc(api, "CheckForUpdates", {});
            await rpc(api, "Update", { update_all: true });
            break;
        }
        case "gyro-target": {
            const d = req.type === undefined ? store.GetController(req.index) : device();
            const mode = gyroMode(d);
            if (!mode) throw new Error(text.selected_controller_s_gyro_is_unavailable);
            if (req.token && !d.strSerialNumber)
                window[gyroPrompt] = { token: req.token, controller: new WeakRef(d) };
            return {nControllerIndex: d.nControllerIndex, eControllerType: d.eControllerType,
                strSerialNumber: d.strSerialNumber ?? "", name: d.strName ?? "", mode};
        }
        case "gyro-start": {
            if (window[gyroSlot]) throw new Error(text.calibration_is_already_running);
            const d = device();
            if (!req.serial && (window[gyroPrompt]?.token !== req.token || window[gyroPrompt].controller.deref() !== d))
                throw new Error(text.steam_refreshed_the_controller_without_a_serial_number);
            if (window[gyroPrompt]?.token === req.token) delete window[gyroPrompt];
            if (gyroMode(d) !== req.mode)
                throw new Error(text.calibration_mode_changed_reopen_the_wizard);
            const api = req.mode === "software" ? manager() : null;
            const args = { controller_index: req.index, side: gyroSide };
            const flow = { controller_index: req.index, flow_mode: 5 };
            const session = {
                token: req.token, controller: d, timer: null, closed: false,
                flowed: false, started: false, observedWork: false, completed: null,
                view: { stage: "waiting" },
                sameDevice() {
                    // Steam can refresh this object during one connection. Check its stable identity.
                    const current = device();
                    if (!req.serial && current !== this.controller)
                        throw new Error(text.steam_refreshed_the_controller_without_a_serial_number);
                    return current;
                },
                renew() {
                    clearTimeout(this.timer);
                    this.timer = setTimeout(() => this.close().catch(console.error), 3500);
                },
                observe(message) {
                    const sample = message.Body().toObject();
                    if (!this.started || this.closed || this.completed || this.view.stage === "failed" ||
                        sample.controller_index !== req.index || sample.imu_index !== gyroSide) return;
                    try {
                        this.sameDevice();
                    } catch (error) {
                        this.view = { stage: "failed", detail: error.message };
                        return;
                    }
                    const progress = Number.isFinite(sample.calibration_progress)
                        ? Math.min(1, Math.max(0, sample.calibration_progress)) : null;
                    switch (sample.calibration_state) {
                        case 1:
                        case 2:
                            this.observedWork = true;
                            this.view = { stage: sample.calibration_state === 1 ? "stationary" : "drift", progress };
                            break;
                        case 3:
                            if (this.observedWork) {
                                this.completed = sample;
                                this.view = { stage: "complete", savable:
                                    finitePositive(sample.accelerometer_noise_tolerance) &&
                                    finitePositive(sample.gyroscope_noise_tolerance) };
                            }
                            break;
                        case 4:
                            this.view = { stage: "failed", detail: text.steam_reported_a_calibration_error, retryable: true };
                            break;
                    }
                },
                close() {
                    if (this.closing) return this.closing;
                    this.closed = true;
                    clearTimeout(this.timer);
                    this.registration?.unregister?.();
                    this.closing = (async () => {
                        try {
                            await this.pending;
                            if (api) {
                                try {
                                    if (this.started) {
                                        this.sameDevice();
                                        await rpc(api, "CancelGyroSoftwareCalibration", args);
                                    }
                                } finally {
                                    if (this.flowed) {
                                        this.sameDevice();
                                        await rpc(api, "EndControllerStateFlow", flow);
                                    }
                                }
                            }
                        } finally {
                            if (window[gyroSlot] === this) delete window[gyroSlot];
                        }
                    })();
                    return this.closing;
                }
            };
            window[gyroSlot] = session;
            session.renew();
            session.pending = (async () => {
                if (api) {
                    session.registration = api.RegisterForNotifyGyroCalibrationStateChanged(message => session.observe(message));
                    if (typeof session.registration?.unregister !== "function")
                        throw new Error(text.steam_did_not_provide_a_calibration_subscription);
                    session.flowed = true;
                    await rpc(api, "StartControllerStateFlow", flow);
                    if (session.closed) return;
                    session.sameDevice();
                    session.started = true;
                    await rpc(api, "StartGyroSoftwareCalibration", args);
                } else {
                    session.startedAt = performance.now();
                    await native("CalibrateControllerIMU", [req.index]);
                }
                session.acknowledged = true;
            })().catch(error => { session.view = { stage: "failed", detail: String(error.message ?? error) }; });
            break;
        }
        case "gyro-poll": {
            const session = gyroSession();
            session.sameDevice();
            session.renew();
            if (session.view.stage === "failed") return session.view;
            if (req.mode === "hardware" && session.startedAt !== undefined) {
                const progress = Math.min(1, (performance.now() - session.startedAt) / (req.type === 10 ? 4000 : 2000));
                return progress === 1 && session.acknowledged
                    ? { stage: "unconfirmed" } : { stage: "hardware", progress };
            }
            if (session.view.stage === "complete" && !session.acknowledged)
                return { stage: "waiting" };
            return session.view;
        }
        case "gyro-save": {
            const session = gyroSession();
            const d = session.sameDevice();
            const sample = session.completed;
            if (!session.acknowledged || session.saving || !sample || !session.view.savable)
                throw new Error(text.steam_did_not_provide_new_calibration_tolerances_to);
            session.saving = true;
            session.renew();
            const oldAccel = d.flAccelerometerStationaryTolerance;
            const oldGyro = d.flGyroStationaryTolerance;
            d.flAccelerometerStationaryTolerance = sample.accelerometer_noise_tolerance;
            d.flGyroStationaryTolerance = sample.gyroscope_noise_tolerance;
            const saving = native("SaveControllerCalibration", [req.index]).catch(error => {
                d.flAccelerometerStationaryTolerance = oldAccel;
                d.flGyroStationaryTolerance = oldGyro;
                throw error;
            });
            session.pending = saving.catch(() => {});
            await saving;
            break;
        }
        case "gyro-end":
            if (window[gyroPrompt]?.token === req.token) delete window[gyroPrompt];
            if (window[gyroSlot]?.token === req.token) await window[gyroSlot].close();
            break;
        case "usb-pair":
            await native("SetSteamControllerDonglePairingMode", [req.value, req.cancel]);
            break;
        default:
            throw new Error(text.unknown_controller_action);
    }
    return true;
})(/*request*/)
