use trox::prelude::*;

fn main() {
    let localizer = Localizer::for_source(
        SourceLocale::from_project_ron(
            include_str!("../trox.ron"),
            include_str!("../terms.ron"),
        )
        .expect("valid source localization configuration"),
    )
    .expect("valid source localizer");
    let count = 2_u32;
    let message = txa(
        plural(
            count,
            [
                one("Draw {count} {noun}."),
                other("Draw {count} {noun}."),
            ],
        ),
        tx_args![
            count,
            noun => counted(TermId::new("unit.card"), count),
        ],
        "Instruction using a counted source term.",
    );
    println!("{}", localizer.resolve_checked(&message).unwrap());
}
