#include "commands.hpp"
#include "gui_json.hpp"
#include "retouch_panel.hpp"
#include "localization.hpp"
#include <QFormLayout>
#include <QLineEdit>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QMessageBox>
#include <QPushButton>
#include <QScrollArea>
#include <QSignalBlocker>
#include <QUuid>
#include <QVBoxLayout>
#include <algorithm>
#include <cmath>
namespace aigle {
RetouchPanel::RetouchPanel(QWidget* parent)
    : QWidget(parent), list_(new QListWidget), createMode_(new QComboBox), spotMode_(new QComboBox),
      status_(new QLabel), selection_(new QLabel) {
    installEventFilter(this);
    list_->installEventFilter(this);
    auto* outer = new QVBoxLayout(this);
    outer->setContentsMargins(0, 0, 0, 0);
    auto* scroll = new QScrollArea;
    scroll->setWidgetResizable(true);
    outer->addWidget(scroll);
    auto* body = new QWidget;
    auto* layout = new QVBoxLayout(body);
    layout->setContentsMargins(16, 12, 16, 16);
    layout->setSpacing(8);
    scroll->setWidget(body);
    auto* heading = new QHBoxLayout;
    auto* title = new QLabel;
    bindTranslation(title, [this, title] { title->setText(tr("Retouch")); });
    heading->addWidget(title, 1);
    auto* done = new QPushButton(tr("Done"));
    bindTranslation(done, [this, done] { done->setText(tr("Done")); });
    heading->addWidget(done);
    layout->addLayout(heading);
    connect(done, &QPushButton::clicked, this, &RetouchPanel::done);
    auto* hint = new QLabel(
        tr("Select an existing correction in the list or on the photo. Drag "
           "its center, source, or radius handle. For a new spot, click for an "
           "automatic source or drag from target to source. Alt+drag places over existing "
           "corrections. Tool shortcuts can be changed in Keyboard shortcuts."));
    bindTranslation(hint, [this, hint] {
        hint->setText(tr("Select an existing correction in the list or on the photo. Drag "
           "its center, source, or radius handle. For a new spot, click for an "
           "automatic source or drag from target to source. Alt+drag places over existing "
           "corrections. Tool shortcuts can be changed in Keyboard shortcuts."));
    });
    hint->setWordWrap(true);
    layout->addWidget(hint);
    list_->setObjectName("retouchCorrections");
    list_->setAccessibleName(tr("Retouch corrections"));
    list_->setMaximumHeight(120);
    layout->addWidget(list_);
    connect(list_, &QListWidget::currentRowChanged, this, [this](int row) {
        if (updating_)
            return;
        if (row < 0) {
            newCorrection();
            return;
        }
        const auto key = list_->item(row)->data(Qt::UserRole).value<RetouchSelection>();
        select(key.kind, key.index);
    });
    auto* buttons = new QHBoxLayout;
    for (int i = 0; i < 3; ++i) {
        auto* b = new QPushButton(QStringList{tr("New"), tr("Delete"), tr("Clear all")}.at(i));
        bindTranslation(b, [this, b, i] { b->setText(QStringList{tr("New"), tr("Delete"), tr("Clear all")}.at(i)); });
        b->setObjectName(QStringList{"retouchNew", "retouchDelete", "retouchClear"}.at(i));
        buttons->addWidget(b);
        connect(b, &QPushButton::clicked, this, [this, i] {
            if (i == 0)
                newCorrection();
            else
                remove(i == 2);
        });
    }
    layout->addLayout(buttons);
    selection_->setObjectName("retouchSelection");
    layout->addWidget(selection_);
    createMode_->setObjectName("retouchCreateMode");
    createMode_->setAccessibleName(tr("New correction type"));
    createMode_->addItems({tr("Heal"), tr("Clone"), tr("Red eye")});
    layout->addWidget(createMode_);
    connect(createMode_, &QComboBox::currentIndexChanged, this, [this] {
        if (updating_)
            return;
        syncControls();
        emit defaultsChanged(defaults());
    });
    spotMode_->setObjectName("retouchSpotMode");
    spotMode_->setAccessibleName(tr("Selected spot mode"));
    spotMode_->addItems({tr("Heal"), tr("Clone")});
    bindTranslation(spotMode_, [this] {
        const QSignalBlocker spotBlock(spotMode_), createBlock(createMode_);
        const QStringList labels{tr("Heal"), tr("Clone"), tr("Red eye")};
        for (int i = 0; i < 3; ++i) createMode_->setItemText(i, labels[i]);
        for (int i = 0; i < 2; ++i) spotMode_->setItemText(i, labels[i]);
        spotMode_->setAccessibleName(tr("Selected spot mode"));
        createMode_->setAccessibleName(tr("New correction type"));
        list_->setAccessibleName(tr("Retouch corrections"));
    });
    layout->addWidget(spotMode_);
    connect(spotMode_, &QComboBox::activated, this,
            [this](int i) { edit("mode", i == 0 ? "heal" : "clone"); });
    struct Def {
        const char* field;
        const char* label;
        double min, max, step, factor;
    };
    const QList<Def> defs{{"radius", QT_TR_NOOP("Radius · % of source long edge"), 0.05, 25, 0.05, 100},
                          {"feather", QT_TR_NOOP("Feather · %"), 0, 100, 1, 1},
                          {"opacity", QT_TR_NOOP("Opacity · %"), 0, 100, 1, 100},
                          {"pupil", QT_TR_NOOP("Pupil tolerance · %"), 0, 100, 1, 1},
                          {"darken", QT_TR_NOOP("Pupil darkening · %"), 0, 100, 1, 1},
                          {"target.x", QT_TR_NOOP("Target X · %"), 0, 100, 0.1, 100},
                          {"target.y", QT_TR_NOOP("Target Y · %"), 0, 100, 0.1, 100},
                          {"source.x", QT_TR_NOOP("Source X · %"), 0, 100, 0.1, 100},
                          {"source.y", QT_TR_NOOP("Source Y · %"), 0, 100, 0.1, 100},
                          {"center.x", QT_TR_NOOP("Center X · %"), 0, 100, 0.1, 100},
                          {"center.y", QT_TR_NOOP("Center Y · %"), 0, 100, 0.1, 100}};
    for (const auto& def : defs) {
        auto* row = new QWidget;
        auto* rows = new QVBoxLayout(row);
        rows->setContentsMargins(0, 0, 0, 0);
        rows->setSpacing(2);
        auto* line = new QHBoxLayout;
        auto* label = new QLabel(tr(def.label));
        label->setWordWrap(true);
        line->addWidget(label, 1);
        auto* number = new QDoubleSpinBox;
        number->setObjectName("retouch." + QString(def.field));
        number->setAccessibleName(tr(def.label));
        number->setRange(def.min, def.max);
        number->setDecimals(def.step < 1 ? 2 : 0);
        number->setSingleStep(def.step);
        number->setKeyboardTracking(false);
        number->setMaximumWidth(76);
        line->addWidget(number);
        rows->addLayout(line);
        label->setBuddy(number);
        auto* slider = new QSlider(Qt::Horizontal);
        slider->setAccessibleName(tr(def.label));
        bindTranslation(row, [this, label, number, slider, source = def.label] {
            const auto text = tr(source);
            label->setText(text);
            number->setAccessibleName(text);
            slider->setAccessibleName(text);
        });
        slider->setRange(0, int(std::round((def.max - def.min) / def.step)));
        rows->addWidget(slider);
        slider->installEventFilter(this);
        layout->addWidget(row);
        controls_.append({def.field, def.factor, row, number, slider});
        connect(slider, &QSlider::sliderPressed, this, [this] {
            cancel();
            if (index_ >= 0) {
                gesture_ = true;
                session_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
            }
        });
        connect(slider, &QSlider::valueChanged, this, [this, number, slider, def](int position) {
            if (updating_)
                return;
            const auto value = def.min + position * def.step;
            const QSignalBlocker block(number);
            number->setValue(value);
            edit(def.field, value / def.factor, slider->isSliderDown(), false);
        });
        connect(slider, &QSlider::sliderReleased, this, [this, number, def] {
            if (gesture_)
                edit(def.field, number->value() / def.factor, false, true);
        });
        connect(number, &QDoubleSpinBox::valueChanged, this, [this, slider, def](double value) {
            if (updating_)
                return;
            {
                const QSignalBlocker block(slider);
                slider->setValue(qRound((value - def.min) / def.step));
            }
            edit(def.field, value / def.factor);
        });
    }
    status_->setObjectName("retouchStatus");
    status_->setTextFormat(Qt::PlainText);
    status_->setWordWrap(true);
    layout->addWidget(status_);
    layout->addStretch();
    timer_.setSingleShot(true);
    timer_.setInterval(40);
    connect(&timer_, &QTimer::timeout, this, [this] {
        if (gesture_ && pending_.has_value()) {
            emit command(*pending_);
            pending_ = {};
        }
    });
    syncControls();
    bindTranslation(this, [this] {
        const QSignalBlocker blocker(list_);
        const auto caption = [this](const QString& kind, int index, const RetouchCorrection& value) {
            return tr("%1 %2").arg(kind == "eye" ? tr("Red eye")
                : value.mode == "clone" ? tr("Clone") : tr("Heal")).arg(index + 1);
        };
        for (int row = 0; row < list_->count(); ++row) {
            auto* item = list_->item(row);
            const auto key = item->data(Qt::UserRole).value<RetouchSelection>();
            const auto kind = key.kind;
            const int index = key.index;
            const auto values = (kind == "eye" ? params_.redEye : params_.spots);
            if (index >= 0 && index < values.size()) item->setText(caption(kind, index, values[index]));
        }
        selection_->setText(index_ < 0 ? tr("New correction")
            : caption(kind_, index_, selection_->property("retouchValue").value<RetouchCorrection>()));
        if (status_->property("previewLabel").toBool())
            status_->setText(tr("Preview · release to apply, Escape to cancel"));
    });
}
RetouchDefaults RetouchPanel::defaults() const {
    auto result = defaultValues_;
    result.tool = createMode_->currentIndex() == 2 ? RetouchDefaults::Tool::RedEye
                : createMode_->currentIndex() == 1 ? RetouchDefaults::Tool::Clone : RetouchDefaults::Tool::Heal;
    return result;
}
commands::Retouch RetouchPanel::request(const QString& action) const {
    return commands::Retouch{.action = action, .document_id = qint64(document_), .revision = qint64(revision_), .session = session_.isEmpty() ? QUuid::createUuid().toString(QUuid::WithoutBraces) : session_, .kind = kind_, .index = index_, .expected = expected_};
}
void RetouchPanel::edit(const QString& field, QJsonValue value, bool preview, bool finish) {
    if (updating_)
        return;
    if (index_ < 0) {
        if (field == "radius") defaultValues_.radius = value.toDouble();
        else if (field == "feather") defaultValues_.feather = value.toDouble();
        else if (field == "opacity") defaultValues_.opacity = value.toDouble();
        else if (field == "pupil") defaultValues_.pupil = value.toDouble();
        else if (field == "darken") defaultValues_.darken = value.toDouble();
        emit defaultsChanged(defaults());
        return;
    }
    if (!preview && !finish)
        session_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    auto r = request("set");
    r.field = field;
    r.value = value;
    r.label = tr("Retouch %1").arg(field);
    if (preview) {
        r.phase = commands::Retouch::Phase::Preview;
        status_->setProperty("previewLabel", true);
        status_->setText(tr("Preview · release to apply, Escape to cancel"));
        pending_ = r;
        if (!timer_.isActive())
            timer_.start();
    } else {
        timer_.stop();
        pending_ = {};
        gesture_ = false;
        status_->setProperty("previewLabel", false);
    status_->clear();
        emit command(r);
    }
}
void RetouchPanel::cancel() {
    timer_.stop();
    pending_ = {};
    if (gesture_) {
        gesture_ = false;
        emit command(commands::RetouchCancel{.session = session_});
    }
    session_.clear();
    status_->setProperty("previewLabel", false);
    status_->clear();
    syncControls();
}
bool RetouchPanel::eventFilter(QObject* object, QEvent* event) {
    const auto cancelGesture = [&] {
        cancel();
        if (auto* slider = qobject_cast<QSlider*>(object)) {
            const QSignalBlocker block(slider);
            slider->setSliderDown(false);
        }
        syncControls();
    };
    bool editingText = false;
    for (auto* widget = QApplication::focusWidget(); widget && widget != this; widget = widget->parentWidget())
        editingText |= qobject_cast<QLineEdit*>(widget) || qobject_cast<QAbstractSpinBox*>(widget);
    QList<LocalShortcuts::Binding> bindings;
    if (!editingText || gesture_) bindings.append(LocalShortcuts::Binding{"tool_cancel", "Esc"});
    if (!editingText && index_ >= 0) {
        bindings.append(LocalShortcuts::Binding{"retouch_delete", "Delete"});
        bindings.append(LocalShortcuts::Binding{"retouch_delete_alternate", "Backspace"});
    }
    if (shortcuts_.handle(event, bindings, [&](const QString& id) {
        if (id != "tool_cancel") remove(false);
        else if (gesture_) cancelGesture();
        else emit done();
    })) return true;
    if (gesture_ && (event->type() == QEvent::FocusOut || event->type() == QEvent::Hide ||
                     event->type() == QEvent::UngrabMouse)) cancelGesture();
    return QWidget::eventFilter(object, event);
}
void RetouchPanel::setRadius(double radius) {
    edit("radius", radius);
    syncControls();
}
void RetouchPanel::beginNew(const QString& tool) {
    newCorrection();
    createMode_->setCurrentIndex(tool == "red_eye" ? 2 : tool == "clone" ? 1 : 0);
    syncControls();
    emit defaultsChanged(defaults());
}
void RetouchPanel::newCorrection() {
    cancel();
    kind_.clear();
    index_ = -1;
    expected_ = {};
    {
        const QSignalBlocker block(list_);
        list_->clearSelection();
        list_->setCurrentRow(-1);
    }
    syncControls();
    emit selected({}, -1);
    emit defaultsChanged(defaults());
}
void RetouchPanel::select(const QString& kind, int index) {
    cancel();
    kind_ = kind;
    index_ = index;
    const auto values = (kind == "eye" ? params_.redEye : params_.spots);
    if (index < 0 || index >= values.size()) {
        newCorrection();
        return;
    }
    expected_ = values.at(index);
    {
        const QSignalBlocker block(list_);
        for (int row = 0; row < list_->count(); ++row) {
            const auto key = list_->item(row)->data(Qt::UserRole).value<RetouchSelection>();
            if (key.kind == kind && key.index == index)
                list_->setCurrentRow(row);
        }
    }
    status_->setProperty("previewLabel", false);
    status_->clear();
    syncControls();
    emit selected(kind, index);
}
void RetouchPanel::syncControls(const RetouchCorrection& preview) {
    updating_ = true;
    findChild<QPushButton*>("retouchDelete")->setEnabled(index_ >= 0);
    findChild<QPushButton*>("retouchClear")
        ->setEnabled(!params_.spots.isEmpty() ||
                     !params_.redEye.isEmpty());
    const auto value = preview.empty() ? (index_ >= 0 ? expected_ : RetouchCorrection{.radius = defaultValues_.radius, .feather = defaultValues_.feather, .opacity = defaultValues_.opacity, .pupil = defaultValues_.pupil, .darken = defaultValues_.darken}) : preview;
    const bool eye = index_ >= 0 ? kind_ == "eye" : createMode_->currentIndex() == 2;
    createMode_->setVisible(index_ < 0);
    if (index_ >= 0) {
        const QSignalBlocker block(createMode_);
        createMode_->setCurrentIndex(eye ? 2 : value.mode == "clone" ? 1 : 0);
    }
    spotMode_->setVisible(index_ >= 0 && !eye);
    {
        const QSignalBlocker block(spotMode_);
        spotMode_->setCurrentIndex(value.mode == "clone" ? 1 : 0);
    }
    selection_->setProperty("retouchValue", QVariant::fromValue(value));
    selection_->setText(index_ < 0 ? tr("New correction")
                                   : tr("%1 %2")
                                         .arg(eye                        ? tr("Red eye")
                                              : value.mode == "clone" ? tr("Clone")
                                                                         : tr("Heal"))
                                         .arg(index_ + 1));
    for (const auto& control : controls_) {
        const auto field = control.field;
        const bool visible =
            field == "radius" ||
            (eye ? (field == "pupil" || field == "darken")
                 : (field == "feather" || field == "opacity")) ||
            (index_ >= 0 && (eye ? field.startsWith("center.")
                                 : (field.startsWith("target.") || field.startsWith("source."))));
        control.row->setVisible(visible);
        if (control.slider->isSliderDown() || control.number->hasFocus())
            continue;
        const QSignalBlocker a(control.number), b(control.slider);
        const auto v = value.field(field) * control.factor;
        control.number->setValue(v);
        control.slider->setValue(
            qRound((v - control.number->minimum()) / control.number->singleStep()));
    }
    updating_ = false;
}
void RetouchPanel::remove(bool all) {
    if (index_ < 0 && !all)
        return;
    if (all && QMessageBox::question(
                   this, tr("Clear retouching"),
                   tr("Remove all spot and red-eye corrections? This can be undone."),
                   QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel) != QMessageBox::Yes)
        return;
    cancel();
    session_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    auto r = request(all ? "clear" : "delete");
    r.label = all ? tr("Clear retouching") : tr("Delete correction");
    emit command(r);
}
void RetouchPanel::setState(const RetouchParameters& params, quint64 document, quint64 revision,
                            const QList<RetouchGuide>& guides) {
    if (document_ != document) {
        reset();
        document_ = document;
    }
    if (revision_ != revision && gesture_)
        cancel();
    const auto previousSpots = params_.spots.size(),
               previousEyes = params_.redEye.size();
    params_ = params;
    revision_ = revision;
    guides_ = guides;
    const auto spots = params.spots, eyes = params.redEye;
    updating_ = true;
    list_->clear();
    for (int kind = 0; kind < 2; ++kind) {
        const auto values = kind ? eyes : spots;
        for (qsizetype i = 0; i < values.size(); ++i) {
            const auto value = values[i];
            auto* item = new QListWidgetItem(tr("%1 %2")
                                                 .arg(kind                       ? tr("Red eye")
                                                      : value.mode == "clone" ? tr("Clone")
                                                                                 : tr("Heal"))
                                                 .arg(i + 1),
                                             list_);
            item->setData(Qt::UserRole, QVariant::fromValue(RetouchSelection{kind ? "eye" : "spot", int(i)}));
        }
    }
    updating_ = false;
    if (spots.size() > previousSpots) {
        kind_ = "spot";
        index_ = int(spots.size() - 1);
    } else if (eyes.size() > previousEyes) {
        kind_ = "eye";
        index_ = int(eyes.size() - 1);
    }
    const auto values = kind_ == "eye" ? eyes : spots;
    if (index_ >= values.size())
        index_ = int(values.size() - 1);
    if (index_ >= 0) {
        expected_ = values[index_];
        {
            const QSignalBlocker block(list_);
            for (int row = 0; row < list_->count(); ++row) {
                const auto key = list_->item(row)->data(Qt::UserRole).value<RetouchSelection>();
                if (key.kind == kind_ && key.index == index_)
                    list_->setCurrentRow(row);
            }
        }
        if (!gesture_)
            syncControls();
        emit selected(kind_, index_);
    } else {
        expected_ = {};
        syncControls();
        emit selected({}, -1);
    }
}
void RetouchPanel::setResult(const RetouchResult& result) {
    if (session_.isEmpty() || result.session != session_)
        return;
    if (result.error.has_value()) {
        cancel();
        setError(*result.error);
        return;
    }
    if (gesture_ && result.guide.has_value())
        syncControls(result.guide->value);
}
void RetouchPanel::setError(const QString& error) {
    status_->setProperty("previewLabel", false);
    status_->setText(error);
}
void RetouchPanel::reset() {
    cancel();
    params_ = {};
    guides_ = {};
    expected_ = {};
    kind_.clear();
    index_ = -1;
    document_ = revision_ = 0;
    list_->clear();
    status_->setProperty("previewLabel", false);
    status_->clear();
    syncControls();
}
} // namespace aigle
