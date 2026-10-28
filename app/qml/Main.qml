import QtCore
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts
import org.diascan

ApplicationWindow {
    id: win
    visible: true
    width: 1360
    height: 860
    minimumWidth: 900
    minimumHeight: 600
    title: "DiaScan"

    Backend { id: backend }

    Settings {
        id: cfg
        category: "scan"
        property int resolution: 3600
        property bool sixteenBit: true
        property int film: 0
        property real dust: 0.5
        property real exposureBias: 0
        property bool autoCrop: true
        property int rotation: 1
        property bool flipH: false
        property bool flipV: false
        property real exposure: 0
        property real brightness: 0
        property real contrast: 0
        property real saturation: 0
        property real cyanRed: 0
        property real magentaGreen: 0
        property real yellowBlue: 0
        property real black: 0
        property real white: 1
        property real gamma: 1
        property bool autoLevels: true
        property bool autoColor: false
        property real sharpen: 0.3
        property real grain: 0
        property bool autoDarkDenoise: true
        property int format: 0
        property int jpegQuality: 92
        property string outDir: StandardPaths.writableLocation(StandardPaths.PicturesLocation) + "/Slides"
        property string baseName: "slide"
        property int batchCount: 50
        property bool skipEmpty: true
    }

    readonly property var numericParams: ["resolution", "sixteenBit", "film", "dust", "exposureBias", "autoCrop",
        "rotation", "flipH", "flipV", "exposure", "brightness", "contrast", "saturation", "cyanRed",
        "magentaGreen", "yellowBlue", "black", "white", "gamma", "autoLevels", "autoColor", "sharpen",
        "grain", "autoDarkDenoise", "format", "jpegQuality", "skipEmpty"]

    function setParam(name, value) {
        cfg[name] = value
        backend.setParam(name, Number(value))
    }

    Component.onCompleted: {
        backend.start()
        for (const n of numericParams)
            backend.setParam(n, Number(cfg[n]))
        backend.setText("outDir", cfg.outDir)
        backend.setText("baseName", cfg.baseName)
        if (autoRun === "browse") backend.browseFolder(qsTr("Choose the folder for scanned images"), cfg.outDir)
    }

    // `--screenshot <file> [delay-ms]` saves the window contents (used for testing/docs).
    Timer {
        readonly property int argIndex: Qt.application.arguments.indexOf("--screenshot")
        running: argIndex >= 0
        interval: argIndex >= 0 && Qt.application.arguments.length > argIndex + 2 ? Number(Qt.application.arguments[argIndex + 2]) : 4000
        onTriggered: page.grabToImage(function (r) {
            r.saveToFile(Qt.application.arguments[argIndex + 1])
            Qt.quit()
        })
    }

    // `--run prescan|scan|magazine:N`: runs one action once connected, then saves a
    // screenshot (if requested) and quits. Used for hardware tests.
    property string autoRun: { const i = Qt.application.arguments.indexOf("--run"); return i >= 0 ? Qt.application.arguments[i + 1] : "" }
    property bool autoStarted: false
    Connections {
        target: backend
        function onConnectedChanged() {
            if (!["prescan", "scan"].includes(win.autoRun) && !win.autoRun.startsWith("magazine:")) return
            if (win.autoStarted || !backend.connected) return
            win.autoStarted = true
            if (win.autoRun === "prescan") backend.prescan()
            else if (win.autoRun === "scan") backend.scan()
            else if (win.autoRun.startsWith("magazine:")) backend.scanMagazine(Number(win.autoRun.split(":")[1]))
        }
        function onFolderChosen(path) {
            console.info("folder chosen:", path)
            cfg.outDir = path
            backend.setText("outDir", path)
        }
        function onFolderPortalUnavailable() { folderDialog.open() }
        function onStatusChanged() { console.info("status:", backend.status) }
        function onNeedsPermissionChanged() {
            if (win.autoRun === "permissions" && backend.needsPermission && !win.autoStarted) {
                win.autoStarted = true
                backend.installPermissions()
            }
        }
        function onPermissionErrorChanged() { if (backend.permissionError) console.info("permission error:", backend.permissionError) }
        function onBusyChanged() {
            if (backend.busy) autoQuit.stop()
            else if (win.autoStarted) autoQuit.start()
        }
    }
    Timer {
        id: autoQuit
        interval: 3000
        onTriggered: {
            const i = Qt.application.arguments.indexOf("--shot")
            if (i >= 0) page.grabToImage(function (r) { r.saveToFile(Qt.application.arguments[i + 1]); Qt.quit() })
            else Qt.quit()
        }
    }

    FolderDialog {
        id: folderDialog
        currentFolder: "file://" + cfg.outDir
        onAccepted: {
            cfg.outDir = selectedFolder.toString().replace("file://", "")
            backend.setText("outDir", cfg.outDir)
        }
    }

    Dialog {
        id: gotoDialog
        title: qsTr("Go to slide")
        standardButtons: Dialog.Ok | Dialog.Cancel
        anchors.centerIn: parent
        modal: true
        SpinBox { id: gotoSpin; from: 1; to: 200; value: Math.max(1, backend.position) ; editable: true }
        onAccepted: backend.gotoSlide(gotoSpin.value)
    }

    Dialog {
        id: magazineDialog
        title: qsTr("Scan magazine")
        standardButtons: Dialog.Ok | Dialog.Cancel
        anchors.centerIn: parent
        modal: true
        ColumnLayout {
            Label {
                text: qsTr("Scans the slide in the scanner and the following ones,\nadvancing the magazine automatically.")
            }
            RowLayout {
                Label { text: qsTr("Number of slides") }
                SpinBox { id: countSpin; from: 1; to: 200; value: cfg.batchCount; editable: true
                    onValueModified: cfg.batchCount = value }
            }
            CheckBox {
                text: qsTr("Skip empty slots (stop after three in a row)")
                checked: cfg.skipEmpty
                onToggled: setParam("skipEmpty", checked)
            }
            Label {
                text: qsTr("Images are saved to %1").arg(cfg.outDir)
                color: palette.placeholderText
            }
        }
        onAccepted: backend.scanMagazine(countSpin.value)
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            spacing: 4
            ToolButton { text: qsTr("◀ Back"); enabled: backend.connected && !backend.busy; onClicked: backend.moveSlides(-1)
                ToolTip.visible: hovered; ToolTip.text: qsTr("Backward one frame") }
            Label {
                text: backend.position > 0 ? qsTr("Slide %1").arg(backend.position) : qsTr("No slide")
                font.bold: true
                Layout.leftMargin: 6; Layout.rightMargin: 6
            }
            ToolButton { text: qsTr("Advance ▶"); enabled: backend.connected && !backend.busy; onClicked: backend.moveSlides(1)
                ToolTip.visible: hovered; ToolTip.text: qsTr("Advance one frame") }
            ToolButton { text: qsTr("Go to…"); enabled: backend.connected && !backend.busy; onClicked: gotoDialog.open() }
            ToolButton { text: qsTr("Reload magazine"); enabled: backend.connected && !backend.busy; onClicked: backend.reloadMagazine() }
            ToolSeparator {}
            ToolButton { text: qsTr("Prescan"); enabled: backend.connected && !backend.busy; onClicked: backend.prescan() }
            ToolButton { text: qsTr("Scan"); highlighted: true; enabled: backend.connected && !backend.busy; onClicked: backend.scan() }
            ToolButton { text: qsTr("Scan magazine…"); enabled: backend.connected && !backend.busy; onClicked: magazineDialog.open() }
            ToolButton { text: qsTr("Cancel"); enabled: backend.busy; onClicked: backend.cancel() }
            Item { Layout.fillWidth: true }
            ToolButton { text: backend.connected ? qsTr("Reconnect") : qsTr("Connect"); enabled: !backend.busy; onClicked: backend.reconnect() }
        }
    }

    Rectangle {
        id: page
        anchors.fill: parent
        color: palette.window

    SplitView {
        id: mainSplit
        anchors.fill: parent

        ColumnLayout {
            SplitView.fillWidth: true
            spacing: 0

            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                color: "#202020"

                Image {
                    id: preview
                    anchors.fill: parent
                    anchors.margins: 12
                    source: backend.previewUrl
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    cache: false
                    smooth: true
                }
                Label {
                    anchors.centerIn: parent
                    visible: preview.status !== Image.Ready
                    color: "#bbbbbb"
                    horizontalAlignment: Text.AlignHCenter
                    text: backend.needsPermission
                          ? qsTr("The scanner was found, but access permission is missing.")
                          : backend.connected
                          ? qsTr("Press Prescan to preview the slide in the scanner,\nor Advance to load the next slide from the magazine.")
                          : qsTr("Waiting for the scanner…\nConnect the slide scanner via USB and switch it on.")
                }
                BusyIndicator {
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 16
                    running: backend.busy
                }
            }

            ListView {
                id: thumbs
                Layout.fillWidth: true
                Layout.preferredHeight: visible ? 96 : 0
                visible: backend.savedFiles.length > 0
                orientation: ListView.Horizontal
                spacing: 6
                clip: true
                model: backend.savedFiles.length > 0 ? backend.savedFiles.split("\n") : []
                onCountChanged: positionViewAtEnd()
                delegate: ItemDelegate {
                    width: 120
                    height: 92
                    contentItem: ColumnLayout {
                        Image {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            source: /\.(jpg|png|jxl)$/.test(modelData) ? modelData : ""
                            sourceSize.height: 80
                            fillMode: Image.PreserveAspectFit
                            asynchronous: true
                        }
                        Label {
                            Layout.fillWidth: true
                            text: modelData.split("/").pop()
                            elide: Text.ElideMiddle
                            font.pointSize: 8
                            horizontalAlignment: Text.AlignHCenter
                        }
                    }
                    onClicked: Qt.openUrlExternally(modelData)
                    ToolTip.visible: hovered
                    ToolTip.text: modelData.replace("file://", "")
                }
            }
        }

        ScrollView {
            SplitView.preferredWidth: 360
            SplitView.minimumWidth: 300
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 16
                x: 8
                spacing: 8

                GroupBox {
                    title: qsTr("Scanner")
                    Layout.fillWidth: true
                    Label {
                        width: parent.width
                        wrapMode: Text.Wrap
                        text: backend.deviceInfo.length > 0 ? backend.deviceInfo : qsTr("Not connected")
                    }
                }

                GroupBox {
                    title: qsTr("Scan settings")
                    Layout.fillWidth: true
                    ColumnLayout {
                        anchors.fill: parent
                        RowLayout {
                            Label { text: qsTr("Resolution") ; Layout.fillWidth: true }
                            ComboBox {
                                id: resCombo
                                model: backend.resolutions.length > 0 ? backend.resolutions.split(",") : ["300", "600", "900", "1200", "1800", "2400", "3600"]
                                displayText: currentText + " dpi"
                                currentIndex: Math.max(0, model.indexOf(String(cfg.resolution)))
                                onActivated: setParam("resolution", Number(currentText))
                            }
                        }
                        RowLayout {
                            Label { text: qsTr("Film type"); Layout.fillWidth: true }
                            ComboBox {
                                model: [qsTr("Slide (positive)"), qsTr("Colour negative"), qsTr("B&W positive"), qsTr("B&W negative")]
                                currentIndex: cfg.film
                                onActivated: setParam("film", currentIndex)
                            }
                        }
                        CheckBox { text: qsTr("16 bit per channel"); checked: cfg.sixteenBit; onToggled: setParam("sixteenBit", checked) }
                        CheckBox { text: qsTr("Auto crop to image area"); checked: cfg.autoCrop; onToggled: setParam("autoCrop", checked) }
                        ParamSlider {
                            label: qsTr("Infrared dust removal") + (backend.infrared ? "" : qsTr(" (not available)"))
                            enabled: backend.infrared && (cfg.film === 0 || cfg.film === 1)
                            from: 0; to: 1; value: cfg.dust; defaultValue: 0.5
                            onChanged: (v) => setParam("dust", v)
                        }
                        Label {
                            visible: cfg.film >= 2
                            text: qsTr("Infrared cleaning is not possible with silver-based B&W film.")
                            wrapMode: Text.Wrap
                            Layout.fillWidth: true
                            color: palette.placeholderText
                        }
                        ParamSlider {
                            label: qsTr("Auto exposure bias (stops)")
                            from: -2; to: 2; value: cfg.exposureBias; defaultValue: 0
                            onChanged: (v) => setParam("exposureBias", v)
                        }
                    }
                }

                GroupBox {
                    title: qsTr("Orientation")
                    Layout.fillWidth: true
                    RowLayout {
                        anchors.fill: parent
                        Button { Layout.fillWidth: true; text: "⟲"; onClicked: setParam("rotation", (cfg.rotation + 3) % 4)
                            ToolTip.visible: hovered; ToolTip.text: qsTr("Rotate 90° left") }
                        Button { Layout.fillWidth: true; text: "⟳"; onClicked: setParam("rotation", (cfg.rotation + 1) % 4)
                            ToolTip.visible: hovered; ToolTip.text: qsTr("Rotate 90° right") }
                        Button { Layout.fillWidth: true; text: "⇋"; checkable: true; checked: cfg.flipH; onToggled: setParam("flipH", checked)
                            ToolTip.visible: hovered; ToolTip.text: qsTr("Flip horizontal") }
                        Button { Layout.fillWidth: true; text: "⇵"; checkable: true; checked: cfg.flipV; onToggled: setParam("flipV", checked)
                            ToolTip.visible: hovered; ToolTip.text: qsTr("Flip vertical") }
                    }
                }

                GroupBox {
                    title: qsTr("Image adjustment")
                    Layout.fillWidth: true
                    ColumnLayout {
                        anchors.fill: parent
                        CheckBox { text: qsTr("Auto levels (auto contrast)"); checked: cfg.autoLevels; onToggled: setParam("autoLevels", checked) }
                        CheckBox { text: qsTr("Auto colour / restore faded colours"); checked: cfg.autoColor; onToggled: setParam("autoColor", checked) }
                        CheckBox {
                            text: qsTr("Automatic noise reduction for dark slides")
                            checked: cfg.autoDarkDenoise
                            onToggled: setParam("autoDarkDenoise", checked)
                            ToolTip.visible: hovered
                            ToolTip.text: qsTr("Dense slides reach the sensor's noise floor. Their noise and line streaks are removed automatically; normal slides are not changed.")
                        }
                        ParamSlider { label: qsTr("Exposure (stops)"); from: -2; to: 2; value: cfg.exposure; onChanged: (v) => setParam("exposure", v) }
                        ParamSlider { label: qsTr("Brightness"); value: cfg.brightness; onChanged: (v) => setParam("brightness", v) }
                        ParamSlider { label: qsTr("Contrast"); value: cfg.contrast; onChanged: (v) => setParam("contrast", v) }
                        ParamSlider { label: qsTr("Saturation"); value: cfg.saturation; onChanged: (v) => setParam("saturation", v) }
                        ParamSlider { label: qsTr("Cyan ↔ Red"); value: cfg.cyanRed; onChanged: (v) => setParam("cyanRed", v) }
                        ParamSlider { label: qsTr("Magenta ↔ Green"); value: cfg.magentaGreen; onChanged: (v) => setParam("magentaGreen", v) }
                        ParamSlider { label: qsTr("Yellow ↔ Blue"); value: cfg.yellowBlue; onChanged: (v) => setParam("yellowBlue", v) }
                        ParamSlider { label: qsTr("Black point"); from: 0; to: 0.5; value: cfg.black; onChanged: (v) => setParam("black", v) }
                        ParamSlider { label: qsTr("White point"); from: 0.5; to: 1; value: cfg.white; defaultValue: 1; onChanged: (v) => setParam("white", v) }
                        ParamSlider { label: qsTr("Midtones (gamma)"); from: 0.4; to: 2.5; value: cfg.gamma; defaultValue: 1; onChanged: (v) => setParam("gamma", v) }
                        ParamSlider { label: qsTr("Sharpen"); from: 0; to: 2; value: cfg.sharpen; defaultValue: 0.3; onChanged: (v) => setParam("sharpen", v) }
                        ParamSlider { label: qsTr("Grain reduction"); from: 0; to: 1; value: cfg.grain; onChanged: (v) => setParam("grain", v) }
                        Button {
                            text: qsTr("Reset adjustments")
                            onClicked: {
                                const d = { exposure: 0, brightness: 0, contrast: 0, saturation: 0, cyanRed: 0, magentaGreen: 0,
                                            yellowBlue: 0, black: 0, white: 1, gamma: 1, sharpen: 0.3, grain: 0 }
                                for (const k in d) setParam(k, d[k])
                            }
                        }
                    }
                }

                GroupBox {
                    title: qsTr("Output")
                    Layout.fillWidth: true
                    ColumnLayout {
                        anchors.fill: parent
                        RowLayout {
                            TextField {
                                Layout.fillWidth: true
                                text: cfg.outDir
                                onEditingFinished: { cfg.outDir = text; backend.setText("outDir", text) }
                            }
                            Button { text: qsTr("Browse…"); onClicked: backend.browseFolder(qsTr("Choose the folder for scanned images"), cfg.outDir) }
                        }
                        RowLayout {
                            Label { text: qsTr("File name") }
                            TextField {
                                Layout.fillWidth: true
                                text: cfg.baseName
                                onEditingFinished: { cfg.baseName = text; backend.setText("baseName", text) }
                            }
                        }
                        Label {
                            text: qsTr("Example: %1_%2").arg(cfg.baseName).arg("001") + "." + ["tif", "tif", "jpg", "png", "jxl", "jxl"][cfg.format]
                            color: palette.placeholderText
                        }
                        RowLayout {
                            Label { text: qsTr("Format"); Layout.fillWidth: true }
                            ComboBox {
                                model: ["TIFF 48-bit", "TIFF 24-bit", "JPEG", "PNG 48-bit", "JPEG XL 16-bit", "JPEG XL 8-bit"]
                                currentIndex: cfg.format
                                onActivated: setParam("format", currentIndex)
                            }
                        }
                        ParamSlider {
                            visible: cfg.format === 2 || cfg.format >= 4
                            label: qsTr("Quality"); from: 50; to: 100; stepSize: 1; decimals: 0
                            value: cfg.jpegQuality; defaultValue: 92
                            onChanged: (v) => setParam("jpegQuality", v)
                        }
                    }
                }
                Item { Layout.preferredHeight: 8 }
            }
        }
    }


        // Shown when the scanner is plugged in but the user may not access it (udev rule missing).
        Rectangle {
            id: permissionPanel
            anchors.fill: parent
            visible: backend.needsPermission
            z: 100
            color: "#99000000"
            MouseArea { anchors.fill: parent }  // block input to the UI underneath

            Pane {
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 640)
                padding: 20

                ColumnLayout {
                    anchors.left: parent.left
                    anchors.right: parent.right
                    spacing: 12
                    Label {
                        text: qsTr("Scanner access")
                        font.bold: true
                        font.pointSize: 14
                    }
                    Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: qsTr("The slide scanner is connected, but your user account is not allowed to access it yet.")
                    }
                    Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: qsTr("DiaScan can install a small system rule (udev) that gives the logged-in user access to "
                                 + "Reflecta/PIE slide scanners. This is needed only once; you will be asked for your password.")
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: backend.permissionError.length > 0
                        wrapMode: Text.Wrap
                        color: "#d9534f"
                        text: backend.permissionError
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: backend.permissionError.length > 0
                        wrapMode: Text.Wrap
                        text: qsTr("You can also install it manually in a terminal:")
                    }
                    TextArea {
                        Layout.fillWidth: true
                        visible: backend.permissionError.length > 0
                        readOnly: true
                        selectByMouse: true
                        wrapMode: TextEdit.WrapAnywhere
                        font.family: "monospace"
                        text: backend.permissionHelp
                    }
                    RowLayout {
                        Layout.alignment: Qt.AlignRight
                        BusyIndicator { running: backend.busy; visible: backend.busy; implicitHeight: 32; implicitWidth: 32 }
                        Button {
                            text: qsTr("Not now")
                            enabled: !backend.busy
                            onClicked: backend.needsPermission = false
                        }
                        Button {
                            text: backend.permissionError.length > 0 ? qsTr("Retry") : qsTr("Install access rule…")
                            highlighted: true
                            enabled: !backend.busy
                            onClicked: backend.installPermissions()
                        }
                    }
                }
            }
        }

    }

    footer: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            Label {
                text: backend.status
                elide: Text.ElideRight
                Layout.fillWidth: true
                color: backend.status.startsWith("Error") ? "#d9534f" : palette.windowText
            }
            ProgressBar {
                visible: backend.busy
                value: backend.progress
                indeterminate: backend.busy && backend.progress <= 0
                Layout.preferredWidth: 240
            }
        }
    }
}
