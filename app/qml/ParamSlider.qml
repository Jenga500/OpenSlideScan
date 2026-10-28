import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Slider bound to a backend parameter; `value` is two-way bound by the parent via Settings.
ColumnLayout {
    id: root
    property string label
    property string param
    property real from: -1
    property real to: 1
    property real stepSize: 0.01
    property real value: 0
    property real defaultValue: 0
    property int decimals: 2
    signal changed(real value)
    spacing: 0
    Layout.fillWidth: true

    RowLayout {
        Layout.fillWidth: true
        Label { text: root.label; Layout.fillWidth: true }
        Label {
            text: root.value.toFixed(root.decimals)
            color: palette.placeholderText
            MouseArea {
                anchors.fill: parent
                onDoubleClicked: root.changed(root.defaultValue)
                ToolTip.visible: containsMouse
                ToolTip.text: qsTr("Double-click to reset")
                hoverEnabled: true
            }
        }
    }
    Slider {
        Layout.fillWidth: true
        from: root.from
        to: root.to
        stepSize: root.stepSize
        value: root.value
        onMoved: root.changed(value)
    }
}
