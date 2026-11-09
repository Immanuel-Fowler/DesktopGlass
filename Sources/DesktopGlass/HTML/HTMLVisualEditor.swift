import AppKit
import SwiftUI
import UniformTypeIdentifiers
import WebKit

struct HTMLVisualEditorView: View {
    @ObservedObject var session: HTMLVisualSession
    @State private var namePrompt: HTMLVisualSession.NeedsName?
    @State private var pendingProp: (String, String)?
    @State private var showSplit = false
    @State private var showGroup = false

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                VisualToolbar(session: session, showSplit: $showSplit, showGroup: $showGroup)
                Divider().opacity(0.25)
                VisualWebHost(webView: session.webView)
            }
            Divider().opacity(0.25)
            InspectorPanel(session: session) { prop, value in
                if let need = session.setProperty(prop, value) { pendingProp = (prop, value); namePrompt = need }
            } requestName: { namePrompt = $0 }
            .frame(width: 310)
        }
        .sheet(isPresented: Binding(get: { namePrompt != nil }, set: { if !$0 { namePrompt = nil } })) {
            NameSheet(kind: namePrompt ?? .newClass, suggestion: session.primary?.tag ?? "") { name in
                if namePrompt == .newClass { session.createClass(name) } else { session.createID(name) }
                if let pp = pendingProp { session.setProperty(pp.0, pp.1) }
                pendingProp = nil
                namePrompt = nil
            } cancel: { namePrompt = nil; pendingProp = nil }
        }
        .sheet(isPresented: $showSplit) { SplitStyleSheet(session: session) { showSplit = false } }
        .sheet(isPresented: $showGroup) { GroupStyleSheet(session: session) { showGroup = false } }
        .onAppear { session.refreshIfSourceChanged() }
    }
}

private struct VisualToolbar: View {
    @ObservedObject var session: HTMLVisualSession
    @Binding var showSplit: Bool
    @Binding var showGroup: Bool

    var body: some View {
        HStack(spacing: 8) {
            Picker("", selection: $session.interact) {
                Label("Select", systemImage: "cursorarrow.rays").tag(false)
                Label("Interact", systemImage: "hand.tap").tag(true)
            }
            .pickerStyle(.segmented).labelsHidden().frame(width: 170)
            .help("Select: click elements to style them (shift-click for more). Interact: use the page normally.")
            if let p = session.primary {
                Text(session.map.label(p.index) + (session.selection.count > 1 ? "  +\(session.selection.count - 1)" : ""))
                    .font(.system(size: 11.5, weight: .medium, design: .monospaced)).lineLimit(1)
                Button { session.selectParent() } label: { Image(systemName: "arrow.up.left.square") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Select the parent element")
            } else {
                Text("Click an element · shift-click for more · double-click text to edit it").font(.system(size: 11.5)).foregroundStyle(.secondary)
            }
            Spacer()
            Button { showSplit = true } label: { Label("Split Style", systemImage: "square.split.2x1").font(.system(size: 12)) }
                .buttonStyle(.plain).disabled((session.primary?.classes.isEmpty ?? true) || inJS)
                .help("Give this element its own copy of a shared class, so you can change it alone")
            Button { showGroup = true } label: { Label("Group", systemImage: "square.stack.3d.up").font(.system(size: 12)) }
                .buttonStyle(.plain).disabled(session.selection.isEmpty || inJS)
                .help("Make a new class shared by the selected elements")
            Divider().frame(height: 16)
            iconButton("plus.square.on.square", "Duplicate element") { session.duplicate() }.disabled(session.primary == nil || inJS)
            iconButton("trash", "Delete element") { session.remove() }.disabled(session.primary == nil || inJS)
            iconButton("arrow.uturn.backward", "Undo") { session.undo() }
            iconButton("arrow.clockwise", "Reload") { session.render() }
        }
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10).frame(height: 36)
    }

    private var inJS: Bool { session.primary?.isDynamic ?? false }

    private func iconButton(_ symbol: String, _ title: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: symbol).frame(width: 24, height: 22) }.buttonStyle(.plain).help(title)
    }
}

@MainActor
private struct VisualWebHost: NSViewRepresentable {
    let webView: WKWebView
    func makeNSView(context: Context) -> NSView { let c = NSView(); attach(c); return c }
    func updateNSView(_ c: NSView, context: Context) { if c.subviews.first !== webView { attach(c) } }
    @MainActor private func attach(_ c: NSView) {
        c.subviews.forEach { $0.removeFromSuperview() }
        webView.frame = c.bounds
        webView.autoresizingMask = [.width, .height]
        c.addSubview(webView)
    }
}

/// Picker over where styles go: linked stylesheets first, then the HTML file (<style>, inline).
struct DestinationPicker: View {
    let destinations: [StyleDestination]
    @Binding var selection: StyleDestination
    var allowInline = true

    var body: some View {
        Picker("Styles go to", selection: $selection) {
            ForEach(destinations.filter { allowInline || $0 != .inline }, id: \.self) { d in
                Label(d.label, systemImage: d.isStylesheet ? "doc.text" : (d == .inline ? "chevron.left.forwardslash.chevron.right" : "curlybraces"))
                    .tag(d)
            }
        }
    }
}

private struct InspectorPanel: View {
    @ObservedObject var session: HTMLVisualSession
    let apply: (String, String) -> Void
    let requestName: (HTMLVisualSession.NeedsName) -> Void

    var body: some View {
        VStack(spacing: 0) {
            if let p = session.primary {
                VStack(alignment: .leading, spacing: 8) {
                    Text(headerLabel(p)).font(.system(size: 12.5, weight: .semibold, design: .monospaced)).lineLimit(1)
                    if p.isDynamic {
                        Label("Created by JavaScript — not in the HTML file, so its styles go in a CSS rule",
                              systemImage: "curlybraces.square")
                            .font(.system(size: 10.5)).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
                    }
                    DestinationPicker(destinations: session.destinations, selection: $session.destination, allowInline: !p.isDynamic)
                    if session.destination != .inline {
                        HStack {
                            Picker("Rule", selection: $session.ruleSelector) {
                                ForEach(p.selectors, id: \.self) { Text($0).tag($0) }
                                if p.selectors.isEmpty { Text("None yet").tag("") }
                            }
                            Menu {
                                Button("New class…") { requestName(.newClass) }
                                Button(p.elementID.isEmpty ? "Set ID…" : "Change ID…") { requestName(.newID) }
                            } label: { Image(systemName: "plus") }
                            .menuStyle(.borderlessButton).frame(width: 26)
                            .disabled(p.isDynamic)
                        }
                        Text(ruleNote(p)).font(.system(size: 10.5)).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
                    } else {
                        Text("Written as style=\"…\" on this element in the HTML").font(.system(size: 10.5)).foregroundStyle(.tertiary)
                    }
                }
                .padding(12)
                Divider().opacity(0.25)
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        if p.text != nil { TextContentEditor(session: session, element: p) }
                        ForEach(VisualProps.groups, id: \.0) { group in
                            VStack(alignment: .leading, spacing: 5) {
                                Text(group.0.uppercased()).font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary)
                                ForEach(group.1) { prop in
                                    PropRow(prop: prop, declared: session.declaredValue(prop.name),
                                            computed: p.computed[prop.name] ?? "", apply: apply)
                                }
                            }
                        }
                    }
                    .padding(12)
                }
                .id("\(p.index)-\(p.isDynamic)-\(p.tag)-\(session.ruleSelector)-\(session.destination.label)")
            } else {
                VStack(spacing: 8) {
                    Image(systemName: "cursorarrow.click.2").font(.system(size: 26, weight: .light)).foregroundStyle(.tertiary)
                    Text("Select an element to edit its styles").font(.system(size: 12)).foregroundStyle(.secondary)
                    if session.destinations.first?.isStylesheet == true {
                        Text("Styles go to \(session.destinations.first!.label) unless you choose otherwise")
                            .font(.system(size: 11)).foregroundStyle(.tertiary)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            if !session.message.isEmpty {
                Divider().opacity(0.25)
                Text(session.message).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
                    .padding(.horizontal, 12).padding(.vertical, 6).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .background(Color.black.opacity(0.04))
    }

    private func headerLabel(_ p: SelectedElement) -> String {
        if !p.isDynamic { return "<\(session.map.label(p.index))>" }
        return "<" + p.tag + (p.elementID.isEmpty ? "" : "#" + p.elementID)
            + (p.classes.isEmpty ? "" : "." + p.classes.prefix(3).joined(separator: ".")) + ">"
    }

    private func ruleNote(_ p: SelectedElement) -> String {
        if session.ruleSelector.isEmpty {
            return p.isDynamic ? "It has no class or id — give it one in your script to style it here"
                               : "This element has no class or id yet — add one with +"
        }
        let shared = session.ruleSelector.hasPrefix(".") ? " Affects every element with this class — use Split Style to change just this one." : ""
        return "Edits \(session.ruleSelector) { … } in \(session.destination.label).\(shared)"
    }
}

private struct PropRow: View {
    let prop: VisualProps.Prop
    let declared: String?
    let computed: String
    let apply: (String, String) -> Void
    @State private var text = ""
    @State private var color = Color.clear

    var body: some View {
        HStack(spacing: 6) {
            Text(prop.name).font(.system(size: 11)).foregroundStyle(declared == nil ? .secondary : .primary)
                .frame(width: 98, alignment: .leading).lineLimit(1)
            TextField(computed, text: $text)
                .textFieldStyle(.roundedBorder).font(.system(size: 11, design: .monospaced))
                .onSubmit { apply(prop.name, text) }
            if prop.isColor {
                ColorPicker("", selection: Binding(get: { color }, set: { c in
                    color = c
                    if let hex = c.hexString { text = hex; apply(prop.name, hex) }
                }), supportsOpacity: true).labelsHidden().frame(width: 26)
            }
            if !prop.options.isEmpty {
                Menu {
                    ForEach(prop.options, id: \.self) { o in Button(o) { text = o; apply(prop.name, o) } }
                    Divider()
                    Button("Clear") { text = ""; apply(prop.name, "") }
                } label: { Image(systemName: "chevron.down") }
                .menuStyle(.borderlessButton).frame(width: 22)
            } else if declared != nil {
                Button { text = ""; apply(prop.name, "") } label: { Image(systemName: "xmark.circle.fill") }
                    .buttonStyle(.plain).foregroundStyle(.tertiary).help("Remove this declaration")
            }
        }
        .onAppear { text = declared ?? "" }
        .onChange(of: declared) { d in text = d ?? "" }
    }
}

private struct TextContentEditor: View {
    @ObservedObject var session: HTMLVisualSession
    let element: SelectedElement
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("TEXT").font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary)
            TextField("Text", text: $text, axis: .vertical)
                .textFieldStyle(.roundedBorder).font(.system(size: 12)).lineLimit(1...5)
                .onSubmit { session.setText(text) }
            Text("Return applies · or double-click the text in the page to edit it there, formatting included")
                .font(.system(size: 10)).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
        }
        .onAppear { text = element.text ?? "" }
        .onChange(of: element.index) { _ in text = element.text ?? "" }
    }
}

private func cleanName(_ s: String) -> String {
    s.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: " ", with: "-")
        .filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
}

private struct NameSheet: View {
    let kind: HTMLVisualSession.NeedsName
    let suggestion: String
    let done: (String) -> Void
    let cancel: () -> Void
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(kind == .newClass ? "New class" : "Element ID").font(.headline)
            Text(kind == .newClass ? "Adds the class to this element; its styles go into a .class rule where you chose."
                                   : "Sets the id on this element; its styles go into a #id rule where you chose.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            TextField(kind == .newClass ? "e.g. card" : "e.g. hero", text: $name).textFieldStyle(.roundedBorder).onSubmit(submit)
            HStack {
                Spacer()
                Button("Cancel", action: cancel).keyboardShortcut(.cancelAction)
                Button("Create", action: submit).keyboardShortcut(.defaultAction).disabled(cleanName(name).isEmpty)
            }
        }
        .padding(20).frame(width: 340)
        .onAppear { name = kind == .newClass ? suggestion : "" }
    }

    private func submit() { let c = cleanName(name); if !c.isEmpty { done(c) } }
}

/// Split Style: one element leaves a shared class's styling behind (or gets a variant on top of it).
private struct SplitStyleSheet: View {
    @ObservedObject var session: HTMLVisualSession
    let close: () -> Void
    @State private var base = ""
    @State private var newName = ""
    @State private var copy = true
    @State private var moveInline = true
    @State private var dest: StyleDestination = .styleBlock

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let p = session.primary {
                Text("Split style for <\(session.map.label(p.index))>").font(.headline)
                Picker("From class", selection: $base) { ForEach(p.classes, id: \.self) { Text("." + $0).tag($0) } }
                TextField("New class name", text: $newName).textFieldStyle(.roundedBorder)
                Picker("", selection: $copy) {
                    Text("Copy .\(base)'s styles into the new class, and use it instead").tag(true)
                    Text("Keep .\(base), add the new class for overrides only").tag(false)
                }
                .pickerStyle(.radioGroup).labelsHidden()
                if !session.inlineDeclarations(p.index).isEmpty {
                    Toggle("Move this element's inline styles into the new class", isOn: $moveInline)
                }
                DestinationPicker(destinations: session.destinations, selection: $dest, allowInline: false)
                Text("The other elements using .\(base) keep their look. Only this element's class attribute changes in the HTML.")
                    .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    Spacer()
                    Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                    Button("Split") {
                        session.splitStyle(baseClass: base, newClass: cleanName(newName), copy: copy, moveInline: moveInline, into: dest)
                        close()
                    }
                    .keyboardShortcut(.defaultAction).disabled(cleanName(newName).isEmpty || base.isEmpty)
                }
            }
        }
        .padding(20).frame(width: 460)
        .onAppear {
            guard let p = session.primary else { return }
            base = session.ruleSelector.hasPrefix(".") ? String(session.ruleSelector.dropFirst()) : (p.classes.first ?? "")
            newName = base.isEmpty ? p.tag : base + "-" + (p.elementID.isEmpty ? "alt" : p.elementID)
            dest = session.destination == .inline ? (session.destinations.first ?? .styleBlock) : session.destination
        }
    }
}

/// Group: a new class shared by all selected elements, with the properties they should have in common.
private struct GroupStyleSheet: View {
    @ObservedObject var session: HTMLVisualSession
    let close: () -> Void
    @State private var name = ""
    @State private var dest: StyleDestination = .styleBlock
    @State private var moveInline = true
    @State private var picked: [String: Bool] = [:]
    @State private var values: [String: String] = [:]
    @State private var showAll = false

    private var shownProps: [String] {
        showAll ? VisualProps.all : VisualProps.all.filter { picked[$0] == true }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Group \(session.selection.count) element\(session.selection.count == 1 ? "" : "s") into a new class").font(.headline)
            Text("Each selected element gets the class; the styles you tick go into one shared rule. Shift-click in the page to select more first.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            TextField("Class name, e.g. card", text: $name).textFieldStyle(.roundedBorder)
            DestinationPicker(destinations: session.destinations, selection: $dest, allowInline: false)
            HStack {
                Text("Shared styles").font(.system(size: 12, weight: .semibold))
                Spacer()
                Toggle("Show all properties", isOn: $showAll).toggleStyle(.checkbox).font(.system(size: 11))
            }
            ScrollView {
                VStack(spacing: 4) {
                    if shownProps.isEmpty {
                        Text("No styles in common yet — tick “Show all properties” to choose some.")
                            .font(.system(size: 11)).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    ForEach(shownProps, id: \.self) { prop in
                        HStack(spacing: 8) {
                            Toggle(prop, isOn: Binding(get: { picked[prop] ?? false }, set: { picked[prop] = $0 }))
                                .toggleStyle(.checkbox).font(.system(size: 11)).frame(width: 150, alignment: .leading)
                            TextField(session.primary?.computed[prop] ?? "", text: Binding(get: { values[prop] ?? "" }, set: {
                                values[prop] = $0
                                if !$0.isEmpty { picked[prop] = true }
                            }))
                            .textFieldStyle(.roundedBorder).font(.system(size: 11, design: .monospaced))
                        }
                    }
                }
            }
            .frame(height: 220)
            Toggle("Remove these properties from the elements' inline styles", isOn: $moveInline).font(.system(size: 12))
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Group") {
                    var props: [String: String] = [:]
                    for (k, on) in picked where on {
                        let v = (values[k] ?? "").trimmingCharacters(in: .whitespaces)
                        props[k] = v.isEmpty ? (session.primary?.computed[k] ?? "") : v
                    }
                    session.groupStyle(className: cleanName(name), properties: props.filter { !$0.value.isEmpty },
                                       moveInline: moveInline, into: dest)
                    close()
                }
                .keyboardShortcut(.defaultAction).disabled(cleanName(name).isEmpty)
            }
        }
        .padding(20).frame(width: 480)
        .onAppear(perform: prefill)
    }

    /// Pre-ticks the inline styles every selected element already shares (same value).
    private func prefill() {
        dest = session.destination == .inline ? (session.destinations.first ?? .styleBlock) : session.destination
        let lists = session.selection.map { Dictionary(session.inlineDeclarations($0.index), uniquingKeysWith: { $1 }) }
        guard let first = lists.first else { return }
        for (k, v) in first where lists.allSatisfy({ $0[k] == v }) {
            picked[k] = true
            values[k] = v
        }
        name = session.primary.map { $0.tag == "div" ? "group" : $0.tag + "-group" } ?? "group"
    }
}

extension Color {
    /// #rrggbb (or #rrggbbaa when translucent), for writing CSS.
    var hexString: String? {
        guard let c = NSColor(self).usingColorSpace(.sRGB) else { return nil }
        let r = Int(round(c.redComponent * 255)), g = Int(round(c.greenComponent * 255)), b = Int(round(c.blueComponent * 255))
        let a = Int(round(c.alphaComponent * 255))
        return a < 255 ? String(format: "#%02x%02x%02x%02x", r, g, b, a) : String(format: "#%02x%02x%02x", r, g, b)
    }
}
