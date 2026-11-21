import CoreAudio
import Foundation
import AVFoundation

struct AudioDeviceInfo: Identifiable, Hashable {
    let objectID: AudioObjectID
    let uid: String
    let name: String
    let hasInput: Bool
    let hasOutput: Bool
    var id: String { uid }
}

struct AudioProcessInfo {
    let objectID: AudioObjectID
    let pid: pid_t
    let bundleID: String
    let isRunningOutput: Bool
}

/// Thin CoreAudio HAL helpers (device list, defaults, process list).
enum DeviceManager {
    private static func address(_ selector: AudioObjectPropertySelector,
                                scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private static func getArray<T>(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector, _: T.Type) -> [T] {
        var a = address(sel)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(obj, &a, 0, nil, &size) == noErr, size > 0 else { return [] }
        let count = Int(size) / MemoryLayout<T>.stride
        let ptr = UnsafeMutablePointer<T>.allocate(capacity: count)
        defer { ptr.deallocate() }
        guard AudioObjectGetPropertyData(obj, &a, 0, nil, &size, ptr) == noErr else { return [] }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(size) / MemoryLayout<T>.stride))
    }

    private static func getValue<T>(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector,
                                    scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal, default def: T) -> T? {
        var a = address(sel, scope: scope)
        var v = def
        var size = UInt32(MemoryLayout<T>.size)
        let st = withUnsafeMutablePointer(to: &v) { AudioObjectGetPropertyData(obj, &a, 0, nil, &size, $0) }
        return st == noErr ? v : nil
    }

    private static func getString(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector) -> String? {
        var a = address(sel)
        var cf: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        let st = withUnsafeMutablePointer(to: &cf) { AudioObjectGetPropertyData(obj, &a, 0, nil, &size, $0) }
        guard st == noErr, let cf else { return nil }
        return cf.takeRetainedValue() as String
    }

    private static func channelCount(_ dev: AudioObjectID, scope: AudioObjectPropertyScope) -> Int {
        var a = address(kAudioDevicePropertyStreamConfiguration, scope: scope)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(dev, &a, 0, nil, &size) == noErr, size > 0 else { return 0 }
        let raw = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { raw.deallocate() }
        guard AudioObjectGetPropertyData(dev, &a, 0, nil, &size, raw) == noErr else { return 0 }
        let list = UnsafeMutableAudioBufferListPointer(raw.assumingMemoryBound(to: AudioBufferList.self))
        return list.reduce(0) { $0 + Int($1.mNumberChannels) }
    }

    // MARK: Devices

    static func devices() -> [AudioDeviceInfo] {
        getArray(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDevices, AudioObjectID.self).compactMap { id in
            guard let uid = getString(id, kAudioDevicePropertyDeviceUID) else { return nil }
            let name = getString(id, kAudioObjectPropertyName) ?? uid
            return AudioDeviceInfo(objectID: id, uid: uid, name: name,
                                   hasInput: channelCount(id, scope: kAudioDevicePropertyScopeInput) > 0,
                                   hasOutput: channelCount(id, scope: kAudioDevicePropertyScopeOutput) > 0)
        }
    }

    static func outputDevices() -> [AudioDeviceInfo] { devices().filter { $0.hasOutput } }
    static func inputDevices() -> [AudioDeviceInfo] { devices().filter { $0.hasInput } }

    static func device(uid: String) -> AudioDeviceInfo? { devices().first { $0.uid == uid } }

    static func defaultOutputDevice() -> AudioDeviceInfo? {
        guard let id: AudioObjectID = getValue(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice, default: AudioObjectID(0)),
              id != 0, let uid = getString(id, kAudioDevicePropertyDeviceUID) else { return nil }
        return AudioDeviceInfo(objectID: id, uid: uid, name: getString(id, kAudioObjectPropertyName) ?? uid, hasInput: false, hasOutput: true)
    }

    /// Looks like a virtual loopback device (BlackHole etc.).
    static func isVirtual(_ d: AudioDeviceInfo) -> Bool {
        let n = d.name.lowercased()
        return n.contains("blackhole") || n.contains("loopback") || n.contains("cable") || n.contains("warptalk")
    }

    // MARK: Audio processes (macOS 14.2+ process objects)

    static func audioProcesses() -> [AudioProcessInfo] {
        // VERIFY: kAudioHardwarePropertyProcessObjectList / kAudioProcessProperty* are macOS 14.2+ symbols.
        let ids = getArray(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyProcessObjectList, AudioObjectID.self)
        return ids.compactMap { id in
            guard let pid: pid_t = getValue(id, kAudioProcessPropertyPID, default: pid_t(0)) else { return nil }
            let bid = getString(id, kAudioProcessPropertyBundleID) ?? ""
            let running: UInt32 = getValue(id, kAudioProcessPropertyIsRunningOutput, default: UInt32(0)) ?? 0
            return AudioProcessInfo(objectID: id, pid: pid, bundleID: bid, isRunningOutput: running != 0)
        }
    }

    /// Audio process objects belonging to `target`: bundle id equal / prefixed with the app's id
    /// (Chrome helpers are `com.google.Chrome.helper...`), or descendants of the app's pid.
    static func processObjects(for target: CaptureTarget) -> [AudioObjectID] {
        let descendants = ProcessTree.descendants(of: target.pid)
        return audioProcesses().filter { p in
            p.bundleID == target.bundleID
                || p.bundleID.hasPrefix(target.bundleID + ".")
                || target.extraAudioPrefixes.contains { p.bundleID.hasPrefix($0) }
                || p.pid == target.pid
                || descendants.contains(p.pid)
        }.map { $0.objectID }
    }
}

/// Parent/child relationship via libproc (no shelling out).
enum ProcessTree {
    static func descendants(of root: pid_t) -> Set<pid_t> {
        let count = proc_listallpids(nil, 0)
        guard count > 0 else { return [] }
        var pids = [pid_t](repeating: 0, count: Int(count) + 64)
        let got = proc_listallpids(&pids, Int32(pids.count * MemoryLayout<pid_t>.size))
        guard got > 0 else { return [] }
        var parent: [pid_t: pid_t] = [:]
        for pid in pids.prefix(Int(got)) where pid > 0 {
            var info = proc_bsdinfo()
            let sz = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size))
            if sz == Int32(MemoryLayout<proc_bsdinfo>.size) { parent[pid] = pid_t(info.pbi_ppid) }
        }
        var result: Set<pid_t> = []
        for (pid, _) in parent {
            var cur = pid
            var hops = 0
            while let p = parent[cur], hops < 16 {
                if p == root { result.insert(pid); break }
                cur = p; hops += 1
            }
        }
        return result
    }
}
