use drawbridge_vpn::app;
fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/logo.png"))
        .expect("bundled app icon is a valid PNG");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([420.0, 760.0])
            .with_min_inner_size([420.0, 760.0])
            .with_max_inner_size([420.0, 760.0])
            .with_resizable(false)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "DrawbridgeVPN",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(app::dark_visuals());
            Ok(Box::new(app::DrawbridgeApp::new()))
        }),
    )
}
