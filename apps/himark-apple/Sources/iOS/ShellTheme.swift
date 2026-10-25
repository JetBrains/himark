// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import CoreText
import SwiftUI
import UIKit

/// The native chrome's half of the iOS theme pair
/// (frontend/editor/assets/theme-ios.json, theme-ios-light.json):
/// the `ink` the engine paints with and the `shell` section no Rust
/// part reads — the bar, the section words, and the ramp they are set
/// in. The files speak physical pixels at the pair's `scale`; every
/// metric here is already in points, so a word SwiftUI sets stands
/// exactly as tall as the engine's next to it. Nothing in the views
/// names a size or a color: it all comes from here.
struct ShellTheme {
    /// One rung of the ramp: the size and tracking in points, the
    /// weight and width on the chrome face's axes.
    struct Role {
        let size: CGFloat
        let weight: CGFloat
        let width: CGFloat
        let tracking: CGFloat
    }

    let name: String

    let ground: Color
    let rule: Color
    let ruleStrong: Color
    let ink: Color
    let ink2: Color
    let ink3: Color
    let ink4: Color
    let live: Color

    let hairline: CGFloat
    let barHeight: CGFloat
    let navHeight: CGFloat
    let inset: CGFloat
    let navInset: CGFloat
    let wordGap: CGFloat
    let countGap: CGFloat
    let actionPad: CGFloat

    let title: Role
    let subtitle: Role
    let word: Role
    let tab: Role
    let tabActive: Role
    let count: Role

    /// The bundled pair's half for the appearance. A bundle without
    /// the file (a misconfigured target) still runs: the dark values
    /// stand in, as every engine part's default is the dark look.
    static func load(dark: Bool) -> ShellTheme {
        let resource = dark ? "theme-ios" : "theme-ios-light"
        guard
            let url = Bundle.main.url(forResource: resource, withExtension: "json"),
            let data = try? Data(contentsOf: url),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let theme = ShellTheme(json: json)
        else {
            NSLog("himark: \(resource).json is not in the bundle — the dark fallback stands")
            return fallback
        }
        return theme
    }

    private init?(json: [String: Any]) {
        guard
            let ui = json["ui"] as? [String: Any],
            let ink = ui["ink"] as? [String: Any],
            let shell = ui["shell"] as? [String: Any],
            let scale = shell["scale"] as? Double, scale > 0
        else { return nil }
        let points = { (key: String) -> CGFloat? in
            (shell[key] as? Double).map { CGFloat($0 / scale) }
        }
        let color = { (key: String) -> Color? in
            (ink[key] as? String).flatMap(Color.init(hex:))
        }
        let role = { (key: String) -> Role? in
            guard
                let rung = shell[key] as? [String: Any],
                let size = rung["size"] as? Double,
                let weight = rung["weight"] as? Double,
                let width = rung["width"] as? Double,
                let tracking = rung["tracking"] as? Double
            else { return nil }
            return Role(
                size: CGFloat(size / scale), weight: CGFloat(weight), width: CGFloat(width),
                tracking: CGFloat(tracking / scale))
        }
        guard
            let ground = color("ground"), let rule = color("rule"),
            let ruleStrong = color("rule_strong"), let ink1 = color("ink"),
            let ink2 = color("ink_2"), let ink3 = color("ink_3"), let ink4 = color("ink_4"),
            let live = color("live"),
            let hairline = points("hairline"), let barHeight = points("bar_height"),
            let navHeight = points("nav_height"), let inset = points("inset"),
            let navInset = points("nav_inset"), let wordGap = points("word_gap"),
            let countGap = points("count_gap"), let actionPad = points("action_pad"),
            let title = role("title"), let subtitle = role("subtitle"), let word = role("word"),
            let tab = role("tab"), let tabActive = role("tab_active"), let count = role("count")
        else { return nil }
        self.name = json["name"] as? String ?? "ios"
        self.ground = ground
        self.rule = rule
        self.ruleStrong = ruleStrong
        self.ink = ink1
        self.ink2 = ink2
        self.ink3 = ink3
        self.ink4 = ink4
        self.live = live
        self.hairline = hairline
        self.barHeight = barHeight
        self.navHeight = navHeight
        self.inset = inset
        self.navInset = navInset
        self.wordGap = wordGap
        self.countGap = countGap
        self.actionPad = actionPad
        self.title = title
        self.subtitle = subtitle
        self.word = word
        self.tab = tab
        self.tabActive = tabActive
        self.count = count
    }

    private init(
        name: String, ground: Color, rule: Color, ruleStrong: Color, ink: Color, ink2: Color,
        ink3: Color, ink4: Color, live: Color, hairline: CGFloat, barHeight: CGFloat,
        navHeight: CGFloat, inset: CGFloat, navInset: CGFloat, wordGap: CGFloat,
        countGap: CGFloat, actionPad: CGFloat, title: Role, subtitle: Role, word: Role,
        tab: Role, tabActive: Role, count: Role
    ) {
        self.name = name
        self.ground = ground
        self.rule = rule
        self.ruleStrong = ruleStrong
        self.ink = ink
        self.ink2 = ink2
        self.ink3 = ink3
        self.ink4 = ink4
        self.live = live
        self.hairline = hairline
        self.barHeight = barHeight
        self.navHeight = navHeight
        self.inset = inset
        self.navInset = navInset
        self.wordGap = wordGap
        self.countGap = countGap
        self.actionPad = actionPad
        self.title = title
        self.subtitle = subtitle
        self.word = word
        self.tab = tab
        self.tabActive = tabActive
        self.count = count
    }

    /// theme-ios.json's values, for a bundle that lost the file.
    static let fallback = ShellTheme(
        name: "ios-dark",
        ground: Color(hex: "#0d0d0f")!, rule: Color(hex: "#222226")!,
        ruleStrong: Color(hex: "#2e2e33")!, ink: Color(hex: "#eceae4")!,
        ink2: Color(hex: "#b5b2ab")!, ink3: Color(hex: "#8a8781")!,
        ink4: Color(hex: "#5f5d59")!, live: Color(hex: "#f2b544")!,
        hairline: 1, barHeight: 44, navHeight: 44, inset: 20, navInset: 4, wordGap: 8,
        countGap: 6, actionPad: 8,
        title: Role(size: 17, weight: 640, width: 100, tracking: 0),
        subtitle: Role(size: 13, weight: 400, width: 100, tracking: 0),
        word: Role(size: 15, weight: 400, width: 100, tracking: 0),
        tab: Role(size: 15, weight: 400, width: 100, tracking: 0),
        tabActive: Role(size: 15, weight: 640, width: 100, tracking: 0),
        count: Role(size: 13, weight: 400, width: 100, tracking: 0))

    /// The role on the chrome face — the bundled variable Bricolage
    /// Grotesque, instanced on its axes like the engine instances it
    /// (`hikit::fonts::chrome_font`): wght, wdth, and opsz at the size.
    func font(_ role: Role) -> Font {
        Font(ChromeFace.font(size: role.size, weight: role.weight, width: role.width))
    }
}

/// The chrome face as UIKit sees it: the variable font registered
/// from the bundle (Info-iOS.plist's UIAppFonts), one instance per
/// (size, weight, width). Without the family — a build that forgot
/// the resource — the system face stands in at the nearest weight so
/// the words still read.
enum ChromeFace {
    static let family = "Bricolage Grotesque"

    private static let wght = 0x7767_6874
    private static let wdth = 0x7764_7468
    private static let opsz = 0x6F70_737A

    private static let registered: Bool = UIFont.familyNames.contains(family)

    static func font(size: CGFloat, weight: CGFloat, width: CGFloat) -> UIFont {
        guard registered else {
            return .systemFont(ofSize: size, weight: systemWeight(weight))
        }
        let variation: [Int: CGFloat] = [
            wght: weight,
            wdth: width,
            opsz: min(max(size, 12), 96),
        ]
        let descriptor = UIFontDescriptor(fontAttributes: [
            .family: family,
            UIFontDescriptor.AttributeName(rawValue: kCTFontVariationAttribute as String): variation,
        ])
        return UIFont(descriptor: descriptor, size: size)
    }

    private static func systemWeight(_ weight: CGFloat) -> UIFont.Weight {
        switch weight {
        case ..<350: .light
        case ..<450: .regular
        case ..<550: .medium
        case ..<650: .semibold
        default: .bold
        }
    }
}

extension Color {
    /// A theme color: `#rrggbb` or `#aarrggbb`, as the engine reads it.
    init?(hex: String) {
        let digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        guard let value = UInt32(digits, radix: 16) else { return nil }
        let (a, r, g, b): (UInt32, UInt32, UInt32, UInt32)
        switch digits.count {
        case 6: (a, r, g, b) = (0xff, value >> 16 & 0xff, value >> 8 & 0xff, value & 0xff)
        case 8: (a, r, g, b) = (value >> 24 & 0xff, value >> 16 & 0xff, value >> 8 & 0xff, value & 0xff)
        default: return nil
        }
        self.init(
            .sRGB, red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255,
            opacity: Double(a) / 255)
    }
}
