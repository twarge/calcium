import Foundation

/// Carries the user's editing preferences between their devices through
/// iCloud's key-value store.
///
/// Settings stay in `UserDefaults`, where every `@AppStorage` reads them; this
/// only copies a fixed list of keys in both directions. What describes one
/// device rather than the person stays put: the font size, the hidden title
/// bar, the chart height. The colours stay put too — an override is cleared
/// by removing its key, and a removal is not something this mirror carries.
@MainActor
final class SettingsSync {
    static let shared = SettingsSync()

    static let syncedKeys: [String] = [
        "proseSystemFont",
        "ligatures",
        "completions",
        "proseSpelling",
        "proseAutocorrect",
        "starterText",
    ]

    private let defaults: UserDefaults
    private let store: NSUbiquitousKeyValueStore
    private var observers: [NSObjectProtocol] = []

    init(defaults: UserDefaults = .standard, store: NSUbiquitousKeyValueStore = .default) {
        self.defaults = defaults
        self.store = store
    }

    /// Begins syncing. Without the iCloud entitlement, or signed out of
    /// iCloud, the store simply never reports or accepts anything.
    func start() {
        guard observers.isEmpty else { return }
        observers.append(NotificationCenter.default.addObserver(
            forName: NSUbiquitousKeyValueStore.didChangeExternallyNotification,
            object: store,
            queue: .main
        ) { [weak self] notification in
            let keys = notification.userInfo?[NSUbiquitousKeyValueStoreChangedKeysKey] as? [String]
            MainActor.assumeIsolated { self?.pull(keys: keys ?? Self.syncedKeys) }
        })
        observers.append(NotificationCenter.default.addObserver(
            forName: UserDefaults.didChangeNotification,
            object: defaults,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.push() }
        })
        store.synchronize()
        pull(keys: Self.syncedKeys)
    }

    /// Cloud to device, for keys the cloud has a value for. Open documents
    /// restyle on the command bus, not by watching defaults, so a change
    /// that arrives from elsewhere is announced the way Preferences would.
    private func pull(keys: [String]) {
        var changed = false
        for key in keys where Self.syncedKeys.contains(key) {
            guard let value = store.object(forKey: key) as? NSObject,
                  (defaults.object(forKey: key) as? NSObject) != value
            else { continue }
            defaults.set(value, forKey: key)
            changed = true
        }
        if changed { CommandBus.shared.send(.preferencesChanged) }
    }

    /// Device to cloud, for keys the user has actually set. A registered
    /// default is not a choice and must not overwrite one made elsewhere.
    private func push() {
        let chosen = defaults.persistentDomain(forName: Bundle.main.bundleIdentifier ?? "") ?? [:]
        for key in Self.syncedKeys {
            guard let value = chosen[key] as? NSObject,
                  (store.object(forKey: key) as? NSObject) != value
            else { continue }
            store.set(value, forKey: key)
        }
    }
}
