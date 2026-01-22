use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::Widget;
use iced::advanced::text::paragraph;
use iced::widget::text as iced_text;
use iced::{alignment, mouse, Color, Element, Length, Pixels, Point, Rectangle, Size};

pub struct EllipsizedText<'a, Theme, Renderer>
where
    Theme: iced_text::Catalog,
    Renderer: text::Renderer,
{
    content: String,
    size: Option<Pixels>,
    line_height: text::LineHeight,
    font: Option<Renderer::Font>,
    color: Option<Color>,
    width: Length,
    height: Length,
    class: Theme::Class<'a>,
}

struct State<P: Paragraph> {
    paragraph: paragraph::Plain<P>,
    ellipsis: paragraph::Plain<P>,
}

impl<P: Paragraph> Default for State<P> {
    fn default() -> Self {
        Self {
            paragraph: paragraph::Plain::default(),
            ellipsis: paragraph::Plain::default(),
        }
    }
}

impl<'a, Theme, Renderer> EllipsizedText<'a, Theme, Renderer>
where
    Theme: iced_text::Catalog,
    Renderer: text::Renderer,
{
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            size: None,
            line_height: text::LineHeight::default(),
            font: None,
            color: None,
            width: Length::Fill,
            height: Length::Shrink,
            class: Theme::default(),
        }
    }

    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = Some(size.into());
        self
    }

    pub fn font(mut self, font: impl Into<Renderer::Font>) -> Self {
        self.font = Some(font.into());
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn line_height(mut self, line_height: impl Into<text::LineHeight>) -> Self {
        self.line_height = line_height.into();
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EllipsizedText<'_, Theme, Renderer>
where
    Theme: iced_text::Catalog,
    Renderer: text::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();

        let limits = limits.width(self.width).height(self.height);
        let bounds = limits.max();

        let size = self.size.unwrap_or_else(|| renderer.default_size());
        let font = self.font.unwrap_or_else(|| renderer.default_font());

        // Measure ellipsis width first
        state.ellipsis.update(text::Text {
            content: "…",
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size,
            line_height: self.line_height,
            font,
            horizontal_alignment: alignment::Horizontal::Left,
            vertical_alignment: alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        });
        let ellipsis_width = state.ellipsis.min_width();

        // Measure full text
        state.paragraph.update(text::Text {
            content: &self.content,
            bounds: Size::new(f32::INFINITY, bounds.height),
            size,
            line_height: self.line_height,
            font,
            horizontal_alignment: alignment::Horizontal::Left,
            vertical_alignment: alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        });

        let full_width = state.paragraph.min_width();

        // Get paragraph bounds
        let para_bounds = state.paragraph.min_bounds();

        // If text fits, use it as-is
        if full_width <= bounds.width {
            let size = Size::new(full_width, para_bounds.height);
            return layout::Node::new(limits.resolve(self.width, self.height, size));
        }

        // Text doesn't fit - find cutoff point using hit_test
        let target_width = bounds.width - ellipsis_width;
        let y_mid = para_bounds.height / 2.0;

        if let Some(hit) = state.paragraph.raw().hit_test(Point::new(target_width, y_mid)) {
            let offset = match hit {
                text::Hit::CharOffset(o) => o,
            };

            // Truncate and add ellipsis - iterate backwards until it fits
            let mut truncated: String = self.content.chars().take(offset).collect::<String>().trim_end().to_string() + "…";

            loop {
                state.paragraph.update(text::Text {
                    content: &truncated,
                    bounds: Size::new(f32::INFINITY, bounds.height),
                    size,
                    line_height: self.line_height,
                    font,
                    horizontal_alignment: alignment::Horizontal::Left,
                    vertical_alignment: alignment::Vertical::Top,
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                });

                if state.paragraph.min_width() <= bounds.width {
                    break;
                }

                // Still too wide, remove one more character
                let chars: Vec<char> = truncated.chars().collect();
                if chars.len() <= 2 {
                    break; // Just ellipsis left
                }
                truncated = chars[..chars.len() - 2].iter().collect::<String>().trim_end().to_string() + "…";
            }
        }

        let final_bounds = state.paragraph.min_bounds();
        let size = Size::new(
            final_bounds.width.min(bounds.width),
            final_bounds.height,
        );
        layout::Node::new(limits.resolve(self.width, self.height, size))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let style = theme.style(&self.class);

        let color = self.color.unwrap_or(style.color.unwrap_or(defaults.text_color));

        renderer.fill_paragraph(
            state.paragraph.raw(),
            layout.position(),
            color,
            *viewport,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<EllipsizedText<'a, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Theme: iced_text::Catalog + 'a,
    Renderer: text::Renderer + 'a,
{
    fn from(text: EllipsizedText<'a, Theme, Renderer>) -> Self {
        Element::new(text)
    }
}

pub fn ellipsized_text<'a, Theme, Renderer>(
    content: impl Into<String>,
) -> EllipsizedText<'a, Theme, Renderer>
where
    Theme: iced_text::Catalog,
    Renderer: text::Renderer,
{
    EllipsizedText::new(content)
}
