mod bridge;
mod engine;
mod logger;
mod portal;
mod udev;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

fn main() {
    logger::init(&engine::cache_dir());
    log::info!("DiaScan {} starting", env!("CARGO_PKG_VERSION"));
    let mut app = QGuiApplication::new();
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_application_name(&QString::from("DiaScan"));
        app.as_mut().set_organization_name(&QString::from("diascan"));
        app.as_mut().set_organization_domain(&QString::from("io.github.diascan"));
    }
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/org/diascan/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
