#include "theme.hpp"
#include <QApplication>
#include <QFile>
#include <QFontDatabase>
#include <QPainter>
#include <QPalette>
#include <QStyleFactory>
#include <QSvgRenderer>
#include <QToolButton>
#include <QVBoxLayout>

namespace aigle {
void applyTheme(QApplication& app) {
    app.setStyle(QStyleFactory::create("Fusion"));
    auto font = QFontDatabase::systemFont(QFontDatabase::GeneralFont);
    font.setPixelSize(13);
    app.setFont(font);
    QPalette palette;
    palette.setColor(QPalette::Window, QColor("#242424"));
    palette.setColor(QPalette::WindowText, QColor("#e6e6e6"));
    palette.setColor(QPalette::Base, QColor("#242424"));
    palette.setColor(QPalette::AlternateBase, QColor("#202020"));
    palette.setColor(QPalette::Text, QColor("#e6e6e6"));
    palette.setColor(QPalette::Button, QColor("#2d2d2d"));
    palette.setColor(QPalette::ButtonText, QColor("#e6e6e6"));
    palette.setColor(QPalette::Highlight, QColor("#343c44"));
    palette.setColor(QPalette::HighlightedText, QColor("#e6e6e6"));
    palette.setColor(QPalette::Mid, QColor("#2d2d2d"));
    palette.setColor(QPalette::Light, QColor("#404040"));
    palette.setColor(QPalette::Dark, QColor("#242424"));
    palette.setColor(QPalette::PlaceholderText, QColor("#b0b0b0"));
    palette.setColor(QPalette::Disabled, QPalette::Text, QColor("#525252"));
    palette.setColor(QPalette::Disabled, QPalette::ButtonText, QColor("#525252"));
    palette.setColor(QPalette::ToolTipBase, QColor("#202020"));
    palette.setColor(QPalette::ToolTipText, QColor("#e6e6e6"));
    app.setPalette(palette);
    QFile style(":/ui/assets/theme.qss");
    if (style.open(QIODevice::ReadOnly))
        app.setStyleSheet(QString::fromUtf8(style.readAll()));
}
QIcon glyph(const QString& name) {
    QSvgRenderer renderer(":/ui/assets/icons/" + name + ".svg");
    QPixmap pixmap(48, 48);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    renderer.render(&painter);
    painter.end();
    pixmap.setDevicePixelRatio(3);
    return QIcon(pixmap);
}
QToolButton* iconButton(const QString& name, const QString& label, QWidget* parent) {
    auto* button = new QToolButton(parent);
    button->setIcon(glyph(name));
    button->setIconSize({16, 16});
    button->setFixedSize(26, 26);
    button->setAutoRaise(true);
    button->setToolTip(label);
    button->setAccessibleName(label);
    return button;
}
QWidget* section(const QString& title, QWidget* body, bool expanded, QWidget* parent) {
    auto* group = new QWidget(parent);
    group->setObjectName("adjustmentGroup");
    group->setProperty("searchText", title);
    auto* layout = new QVBoxLayout(group);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    auto* header = new QToolButton(group);
    header->setObjectName("groupHeader");
    header->setText(QString(title).replace("&", "&&"));
    header->setCheckable(true);
    header->setChecked(expanded);
    header->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    header->setArrowType(expanded ? Qt::DownArrow : Qt::RightArrow);
    header->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    header->setFixedHeight(39);
    layout->addWidget(header);
    layout->addWidget(body);
    body->setVisible(expanded);
    QObject::connect(header, &QToolButton::toggled, body, [body, header](bool on) {
        body->setVisible(on);
        header->setArrowType(on ? Qt::DownArrow : Qt::RightArrow);
    });
    return group;
}
} // namespace aigle
