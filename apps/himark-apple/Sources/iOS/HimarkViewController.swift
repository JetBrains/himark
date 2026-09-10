import UIKit

final class HimarkViewController: UIViewController, UIDocumentPickerDelegate {
    private let himarkView = HimarkView(frame: .zero)

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        himarkView.installHost(presenter: self)

        himarkView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(himarkView)

        let toolbar = UIToolbar()
        toolbar.translatesAutoresizingMaskIntoConstraints = false

        let burger = UIBarButtonItem(
            image: UIImage(systemName: "line.3.horizontal"),
            menu: UIMenu(children: [
                UIAction(title: "New", image: UIImage(systemName: "doc")) { [weak self] _ in
                    self?.himarkView.newScratch()
                },
                UIAction(title: "Open…", image: UIImage(systemName: "folder")) { [weak self] _ in
                    self?.himarkView.openFilePicker()
                },
                UIAction(title: "Save…", image: UIImage(systemName: "square.and.arrow.down")) {
                    [weak self] _ in self?.saveDocument()
                },
                UIAction(title: "Open Demo", image: UIImage(systemName: "sparkles")) {
                    [weak self] _ in self?.himarkView.openDemo()
                },

                UIAction(title: "Sessions", image: UIImage(systemName: "person.2")) {
                    [weak self] _ in self?.himarkView.toggleAgents()
                },
                UIMenu(options: .displayInline, children: [
                    UIAction(title: "Peeker", image: UIImage(systemName: "filemenu.and.selection")) {
                        [weak self] _ in self?.himarkView.togglePeeker()
                    },
                    UIAction(title: "Workspace Tree", image: UIImage(systemName: "sidebar.left")) {
                        [weak self] _ in self?.himarkView.toggleWorkspaceTree()
                    },
                    UIAction(title: "Switch Workspace", image: UIImage(systemName: "square.on.square")) {
                        [weak self] _ in self?.himarkView.toggleWorkspaceSwitcher()
                    },
                    UIAction(title: "Split Pane", image: UIImage(systemName: "rectangle.split.2x1")) {
                        [weak self] _ in self?.himarkView.splitPane()
                    },
                    UIAction(title: "Close Pane", image: UIImage(systemName: "xmark.rectangle")) {
                        [weak self] _ in self?.himarkView.closePane()
                    },
                ]),
            ])
        )
        let find = UIBarButtonItem(
            image: UIImage(systemName: "magnifyingglass"),
            primaryAction: UIAction { [weak self] _ in self?.himarkView.openSearch() }
        )

        let hideKeyboard = UIBarButtonItem(
            image: UIImage(systemName: "keyboard.chevron.compact.down"),
            primaryAction: UIAction { [weak self] _ in self?.himarkView.dismissKeyboard() }
        )
        toolbar.items = [burger, .flexibleSpace(), find, hideKeyboard]
        view.addSubview(toolbar)

        NSLayoutConstraint.activate([
            himarkView.topAnchor.constraint(equalTo: view.topAnchor),
            himarkView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            himarkView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            himarkView.bottomAnchor.constraint(equalTo: toolbar.topAnchor),
            toolbar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            toolbar.trailingAnchor.constraint(equalTo: view.trailingAnchor),

            toolbar.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
        ])

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(keyboardChanged),
                           name: UIResponder.keyboardDidShowNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardChanged),
                           name: UIResponder.keyboardDidChangeFrameNotification, object: nil)
    }

    @objc private func keyboardChanged() {
        view.layoutIfNeeded()
        himarkView.revealCaretAfterKeyboardChange()
    }

    private func item(_ title: String, _ action: Selector) -> UIBarButtonItem {
        UIBarButtonItem(title: title, style: .plain, target: self, action: action)
    }

    @objc private func newScratch() { himarkView.newScratch() }
    @objc private func openDemo() { himarkView.openDemo() }
    @objc private func togglePeeker() { himarkView.togglePeeker() }
    @objc private func splitPane() { himarkView.splitPane() }
    @objc private func openSearch() { himarkView.openSearch() }
    @objc private func closePane() { himarkView.closePane() }

    @objc private func saveDocument() {
        guard let text = himarkView.documentText() else { return }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("document.md")
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            return
        }
        let picker = UIDocumentPickerViewController(forExporting: [url])
        picker.delegate = self
        present(picker, animated: true)
    }
}
