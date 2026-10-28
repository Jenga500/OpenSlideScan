use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(QmlModule::new("org.diascan").qml_files(["qml/Main.qml", "qml/ParamSlider.qml"]))
        .files(["src/bridge.rs"])
        .build();
}
